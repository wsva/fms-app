#!/usr/bin/env node
// Build the updater manifest (`latest.json`) for one release.
//
// Tauri's updater reads this file from the configured endpoint and installs the
// artifact whose entry matches the running target/arch. It is generated here
// rather than by `tauri-action` because the release assets are renamed to
// `FmS_<platform>_<arch>_<ref>.<ext>` after the build (`build.yml`), and the
// manifest has to point at those final names.
//
// The signature is not a checksum: it is the *content* of the `.sig` file the
// bundler wrote next to each installer, which the client verifies against the
// minisign public key in `tauri.conf.json`. A path or URL there is rejected by
// the plugin, so the bytes are inlined.
//
// Inputs are read off the `.sig` files rather than a hardcoded asset list: a
// `.sig` only exists for a package the updater can actually install, which is
// exactly the set this file may advertise.
//
// Usage (CI uses the environment defaults):
//   node scripts/make-latest-json.mjs --artifacts artifacts \
//     --tag v0.1.9 --repo wsva/fms-app
//
// Written to stdout as well as `<artifacts>/latest.json`, so the publish job can
// echo it into the log.

import { readFileSync, writeFileSync, existsSync, readdirSync } from "node:fs";
import { join } from "node:path";

function arg(name, fallback) {
  const index = process.argv.indexOf(`--${name}`);
  if (index !== -1 && process.argv[index + 1]) return process.argv[index + 1];
  return fallback;
}

const artifactsDir = arg("artifacts", "artifacts");
const tag = arg("tag", process.env.GITHUB_REF_NAME ?? "");
const repo = arg("repo", process.env.GITHUB_REPOSITORY ?? "wsva/fms-app");
const outFile = arg("out", join(artifactsDir, "latest.json"));
const notes = arg("notes", "");

if (!tag) {
  console.error("[latest.json] no --tag and no GITHUB_REF_NAME; nothing to stamp");
  process.exit(1);
}
if (!existsSync(artifactsDir)) {
  console.error(`[latest.json] ${artifactsDir} does not exist`);
  process.exit(1);
}

// The manifest's `version` is a plain semver; the tag is a `vX.Y.Z` ref.
const version = arg("version", tag.replace(/^v/, ""));
const downloadBase = `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}`;

/**
 * Which `platforms` key an installer payload answers to. Tauri spells the key
 * `<os>-<arch>`, with the OS as the updater's `{{target}}` value.
 */
function platformKey(asset) {
  // Windows and Linux reuse the installer itself as the update payload (v2
  // `createUpdaterArtifacts: true`); macOS would be the `.app.tar.gz`, which
  // this release does not publish because the bundle is unsigned.
  if (asset.endsWith(".exe")) return "windows-x86_64";
  if (asset.endsWith(".AppImage")) {
    // `FmS_linux_amd64_v0.1.9.AppImage` — the arch token is the deb/rpm
    // spelling (amd64/arm64), the manifest wants the Rust one.
    if (asset.includes("_arm64_")) return "linux-aarch64";
    return "linux-x86_64";
  }
  if (asset.endsWith(".tar.gz")) return "darwin-aarch64";
  return null;
}

const files = readdirSync(artifactsDir);
const platforms = {};
const skipped = [];

for (const entry of files.filter((name) => name.endsWith(".sig"))) {
  const asset = entry.slice(0, -".sig".length);
  const key = platformKey(asset);
  if (!key || !existsSync(join(artifactsDir, asset))) {
    // A `.sig` without its payload, or a payload this manifest does not carry
    // yet (the macOS `.app.tar.gz`, whose bundle is unsigned).
    skipped.push(entry);
    continue;
  }
  platforms[key] = {
    signature: readFileSync(join(artifactsDir, entry), "utf8").trim(),
    url: `${downloadBase}/${encodeURIComponent(asset)}`,
  };
  console.log(`[latest.json] ${key} <- ${asset}`);
}

if (skipped.length > 0) {
  console.log(`[latest.json] not advertised: ${skipped.join(", ")}`);
}

// Tauri validates the whole file before comparing versions, so a manifest with
// no installable platform is worse than none at all: it would leave every
// client on an error path. Fail instead, on the release that produced nothing.
if (Object.keys(platforms).length === 0) {
  console.error(`[latest.json] no installable artifact in ${artifactsDir}; refusing to write a manifest`);
  process.exit(1);
}

const manifest = {
  version,
  notes: notes || `FmS ${version}: https://github.com/${repo}/releases/tag/${tag}`,
  pub_date: new Date().toISOString(),
  platforms,
};

const body = `${JSON.stringify(manifest, null, 2)}\n`;
writeFileSync(outFile, body);
console.log(body);
console.log(`[latest.json] wrote ${outFile}`);
