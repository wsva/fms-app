use std::process::Command;

fn main() {
    emit_git_describe();
    embed_test_manifest();
    tauri_build::build()
}

/// Give `cargo test` harnesses the application manifest the app exe gets from
/// tauri-build.
///
/// On Windows the Tauri stack statically imports `comctl32!TaskDialogIndirect`,
/// which only exists in the Common-Controls v6 side-by-side assembly. The app
/// exe is linked with a manifest asking for v6 (inside the `resource.lib` that
/// `tauri_build::build()` passes to bins), so the loader redirects it; a test
/// harness is a separate exe linked without any manifest, so it binds System32's
/// v5.82 `comctl32.dll`, cannot find the export, and dies with
/// `STATUS_ENTRYPOINT_NOT_FOUND` (0xc0000139) before the first test runs.
///
/// It cannot simply be always-on: link args from a build script apply to every
/// target of the crate, and the lib's own unit tests have no test-only hook
/// (`rustc-link-arg-tests` covers `tests/` only). A generated manifest on the
/// app bin collides with the one in `resource.lib` — `CVT1100: duplicate
/// resource. type:MANIFEST, name:1`. So the manifest is opt-in for test runs:
///
/// ```text
/// FMS_TEST_MANIFEST=1 cargo test --lib
/// ```
///
/// `--lib` matters: it keeps the bin out of the link, so only the harness picks
/// the manifest up. A plain build never sets the variable and is unaffected.
fn embed_test_manifest() {
    println!("cargo:rerun-if-env-changed=FMS_TEST_MANIFEST");
    if std::env::var_os("FMS_TEST_MANIFEST").is_none()
        || std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
    {
        return;
    }
    let out_dir = std::path::PathBuf::from(
        std::env::var("OUT_DIR").expect("cargo always sets OUT_DIR for build scripts"),
    );
    let manifest = out_dir.join("fms-test-harness.manifest");
    let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#;
    if std::fs::write(&manifest, xml).is_err() {
        println!("cargo:warning=could not write the test-harness manifest; `cargo test` will fail to start on Windows");
        return;
    }
    // A harness links without a manifest at all, so both halves are needed:
    // `/MANIFEST:EMBED` turns the generated manifest on, `/MANIFESTINPUT` puts
    // the dependency inside it.
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg=/MANIFESTINPUT:{}",
        manifest.display()
    );
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
