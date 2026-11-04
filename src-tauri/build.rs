use std::path::{Path, PathBuf};
use std::process::Command;
use std::fs;

fn main() {
    emit_git_describe();
    embed_test_manifest();
    pack_user_manual();
    tauri_build::build()
}

/// Pack `docs/manual` into `$OUT_DIR/manual.tar.gz` for the crate to embed.
///
/// The user manual ships *as app content*, not as a dataset the user imports:
/// baking the bytes into the binary means the shipped manual is always the
/// manual of the running app version (so the in-app reader and the agent both
/// answer from version-matched prose), it needs no `bundle.resources` entry and
/// no runtime `resource_dir` resolution — which in this project resolves to
/// `target/debug` under `tauri dev` and would behave differently in every build
/// mode — and it reaches all five CI targets through the same code path.
///
/// The allowlist is the contract: only `info.json` and `*.md` are packed. A
/// stray `fts5.sqlite3` / `data.sqlite3` from a dev build that scanned the
/// folder, or a `meta.json` of linked locations, are derived or machine-local
/// and would otherwise ride in every installer.
///
/// A hard failure is deliberate: `include_bytes!` of a stale or empty archive
/// would ship an app whose manual silently never updates.
fn pack_user_manual() {
    // The directory form re-runs on any add/remove inside it; content edits to
    // the packed files are covered because `rerun-if-changed` on a directory
    // tracks its mtime, which every write to a child updates.
    println!("cargo:rerun-if-changed=../docs/manual");

    let src = PathBuf::from("../docs/manual");
    let info_path = src.join("info.json");
    let info_raw = fs::read(&info_path).unwrap_or_else(|e| {
        panic!(
            "built-in user manual: cannot read {} — the app ships this directory as its own \
             manual, so it must exist and be readable ({e}).",
            info_path.display()
        )
    });
    let info: serde_json::Value = serde_json::from_slice(&info_raw)
        .unwrap_or_else(|e| panic!("built-in user manual: {} is not valid JSON ({e}).", info_path.display()));
    for key in ["uuid", "updated_at"] {
        if info.get(key).and_then(|v| v.as_str()).unwrap_or("").is_empty() {
            panic!(
                "built-in user manual: {} must carry a non-empty `{key}` — the seed step compares \
                 it against the copy on disk to decide whether to replace it.",
                info_path.display()
            );
        }
    }

    let out_dir =
        PathBuf::from(std::env::var("OUT_DIR").expect("cargo always sets OUT_DIR for build scripts"));
    let archive = out_dir.join("manual.tar.gz");
    let file = fs::File::create(&archive)
        .unwrap_or_else(|e| panic!("built-in user manual: cannot write {}: {e}", archive.display()));

    let mut files = Vec::new();
    collect_packed(&src, &mut files);
    if files.len() <= 1 {
        panic!(
            "built-in user manual: {} holds no markdown — packing an empty manual would ship an \
             app with a Wiki page that has nothing in it.",
            src.display()
        );
    }
    // Sorted so the archive is byte-stable across builds that change nothing.
    files.sort();

    let enc = flate2::write::GzEncoder::new(file, flate2::Compression::new(6));
    let mut builder = tar::Builder::new(enc);
    for path in &files {
        let rel = path
            .strip_prefix(&src)
            .unwrap_or_else(|_| panic!("packed file {} escapes {}", path.display(), src.display()));
        builder
            .append_path_with_name(path, rel)
            .unwrap_or_else(|e| panic!("built-in user manual: cannot pack {}: {e}", path.display()));
    }
    let enc = builder
        .into_inner()
        .unwrap_or_else(|e| panic!("built-in user manual: archive finalize failed: {e}"));
    enc
        .finish()
        .unwrap_or_else(|e| panic!("built-in user manual: gzip write failed: {e}"));
}

/// Every file under `dir` that belongs in the shipped manual, recursively.
fn collect_packed(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if path.is_dir() {
            collect_packed(&path, out);
            continue;
        }
        if name.starts_with('.') {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if name == "info.json" || lower.ends_with(".md") {
            out.push(path);
        }
    }
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
