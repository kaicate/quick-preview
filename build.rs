use std::{env, fs, path::PathBuf, process::Command};

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

    println!("cargo:rerun-if-changed={}", icon_path.display());

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let resource_script = out_dir.join("QuickPreview.rc");
    let compiled_resource = out_dir.join("QuickPreview.res");
    let escaped_icon_path = icon_path.to_string_lossy().replace('\\', "\\\\");

    fs::write(
        &resource_script,
        format!("1 ICON \"{escaped_icon_path}\"\n"),
    )
    .expect("could not write the Windows resource script");

    let output = Command::new(resolve_resource_compiler())
        .arg("/nologo")
        .arg("/fo")
        .arg(&compiled_resource)
        .arg(&resource_script)
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
