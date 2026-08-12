#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod windows_app;

#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    windows_app::run()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("QuickPreview is a Windows 11 application. Run `cargo test` to test its portable document core.");
}
