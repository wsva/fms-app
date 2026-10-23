use std::process::Command;

fn main() {
    emit_git_describe();
    tauri_build::build()
}

/// Bake `git describe --tags` into the binary as `FMS_GIT_DESCRIBE` so the app can
/// report the release tag it was built from — the committed manifest `0.1.0` is only
/// a dev placeholder, and CI stamps the real tag into Cargo.toml instead.
///
/// Deliberately no `--always`: on a shallow, tag-less CI checkout (the default
/// `actions/checkout@v4`) `git describe` fails and we emit nothing, so released
/// builds fall back to the stamped `CARGO_PKG_VERSION` rather than a bare commit hash.
fn emit_git_describe() {
    // Re-run when HEAD moves so the baked version tracks the checked-out commit.
    println!("cargo:rerun-if-changed=../.git/HEAD");
    let describe = Command::new("git")
        .args(["describe", "--tags", "--dirty"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|v| !v.is_empty());
    if let Some(v) = describe {
        println!("cargo:rustc-env=FMS_GIT_DESCRIBE={v}");
    }
}
