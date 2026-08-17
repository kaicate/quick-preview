use std::{env, fs, path::PathBuf, process::Command};

fn create_icon(source: &PathBuf, destination: &PathBuf) {
    let image = image::open(source)
        .unwrap_or_else(|error| panic!("could not load {}: {error}", source.display()))
        .resize_exact(256, 256, image::imageops::FilterType::Lanczos3);
    image
        .save_with_format(destination, image::ImageFormat::Ico)
        .unwrap_or_else(|error| panic!("could not write {}: {error}", destination.display()));
}

fn resource_compiler_path(path: &PathBuf) -> String {
    let value = path.to_string_lossy();
    #[cfg(unix)]
    if let Some(rest) = value.strip_prefix("/mnt/") {
        if rest.as_bytes().get(1) == Some(&b'/') {
            let drive = rest[..1].to_ascii_uppercase();
            return format!("{drive}:\\{}", rest[2..].replace('/', "\\"));
        }
    }
    value.into_owned()
}

fn resolve_resource_compiler() -> PathBuf {
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            let candidate = directory.join("rc.exe");
            if candidate.is_file() {
                return candidate;
            }
        }
    }

    if let Some(program_files) = env::var_os("ProgramFiles(x86)") {
        let sdk_bin = PathBuf::from(program_files)
            .join("Windows Kits")
            .join("10")
            .join("bin");
        let mut candidates = fs::read_dir(sdk_bin)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path().join("x64").join("rc.exe"))
            .filter(|candidate| candidate.is_file())
            .collect::<Vec<_>>();
        candidates.sort();
        if let Some(candidate) = candidates.pop() {
            return candidate;
        }
    }

    panic!("rc.exe was not found; install the Windows SDK or build from a Developer shell");
}

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let icon_path = manifest_dir.join("assets").join("QuickPreview.ico");
    let file_icons = [
        (101, "csv_icon.png"),
        (102, "tsv_icon.png"),
        (103, "markdown_icon.png"),
        (104, "html_icon.png"),
    ];

    println!("cargo:rerun-if-changed={}", icon_path.display());
    for (_, icon) in file_icons {
        println!(
            "cargo:rerun-if-changed={}",
            manifest_dir.join("assets").join(icon).display()
        );
    }

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let resource_script = out_dir.join("QuickPreview.rc");
    let compiled_resource = out_dir.join("QuickPreview.res");
    let escaped_icon_path = resource_compiler_path(&icon_path).replace('\\', "\\\\");
    let mut resource_contents = format!("1 ICON \"{escaped_icon_path}\"\n");
    for (resource_id, file_name) in file_icons {
        let generated = out_dir.join(format!("{resource_id}.ico"));
        create_icon(&manifest_dir.join("assets").join(file_name), &generated);
        let generated = resource_compiler_path(&generated).replace('\\', "\\\\");
        resource_contents.push_str(&format!("{resource_id} ICON \"{generated}\"\n"));
    }

    fs::write(&resource_script, resource_contents)
        .expect("could not write the Windows resource script");

    let output = Command::new(resolve_resource_compiler())
        .arg("/nologo")
        .arg("/fo")
        .arg(resource_compiler_path(&compiled_resource))
        .arg(resource_compiler_path(&resource_script))
        .output()
        .expect("could not run rc.exe");

    if !output.status.success() {
        panic!(
            "rc.exe failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    println!(
        "cargo:rustc-link-arg-bin=QuickPreview={}",
        compiled_resource.display()
    );
}
