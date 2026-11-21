use std::process::Command;

fn main() {
    embuild::espidf::sysenv::output();

    // Firmware version shown on the setup page and in the console `status`:
    // `git describe` (tag, or short commit hash; "-dirty" for uncommitted
    // changes), falling back to the Cargo package version without git.
    let describe = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let version = describe.unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=FW_VERSION={version}");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=.git/refs");
}
