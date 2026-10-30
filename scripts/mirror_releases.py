#!/usr/bin/env python3
"""Mirror the newest stable FmS GitHub release onto the website volume.

Runs hourly from cron on the website node. It reads GitHub; it never writes
to it — build.yml and make-latest-json.mjs stay the single publish path.

    python3 mirror_releases.py [--dry-run] [--root DIR] [--base-url URL]

What it produces under --root (default /wsva/data/fms_mirror):

    <tag>/FmS_*              the installer assets, named exactly as on GitHub
    latest.json              the Tauri updater manifest, urls rewritten to this
                             site; signatures pass through untouched, so the
                             client verifies mirror bytes against the same
                             pinned minisig
    releases.json            website metadata: tag, version, per-asset
                             size/sha256/platform — written LAST, so the site
                             never lists a file that is not fully present

The rewrite is safe because a manifest is just {version, notes, platforms}:
the signature covers the artifact bytes, never the URL. GitHub publishes
latest.json on tag builds only, so a rolling latest-build pre-release has no
manifest to mirror and is skipped — a preview never advertises itself as an
update.

Ordering rule: assets land first (.part + size check + os.replace), then the
rewritten manifest, then releases.json. A kill mid-download leaves only a
stray .part behind; the published files keep pointing at complete ones.
"""

import argparse
import hashlib
import json
import logging
import os
import re
import shutil
import sys
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

DEFAULT_REPO = "wsva/fms-app"
DEFAULT_BASE_URL = "https://lusworkshop.site"
DEFAULT_ROOT = "/wsva/data/fms_mirror"
DEFAULT_TOKEN_FILE = "/etc/fms-mirror.env"
DEFAULT_LOG_FILE = "/var/log/fms-mirror.log"
GITHUB_DL_RE = re.compile(r"https://github\.com/[^/]+/[^/]+/releases/download/[^/]+/")
# The renamed release assets build.yml publishes. .sig files travel with them
# but are not downloadables of their own, and latest.json is fetched whole.
ASSET_RE = re.compile(r"^FmS_.*\.(exe|AppImage|deb|rpm|dmg|apk|ipa)$")
KEEP_TAG_DIRS = 2
STALE_AFTER = 24 * 3600
FAIL_ALERT_AT = 3
DOWNLOAD_CHUNK = 1 << 20


def log_setup(log_file: str, verbose: bool) -> None:
    handlers: list[logging.Handler] = [logging.StreamHandler()]
    try:
        handlers.insert(0, logging.FileHandler(log_file))
    except OSError:
        pass  # a dev machine without /var/log write access still gets stderr
    logging.basicConfig(
        level=logging.DEBUG if verbose else logging.INFO,
        format="%(asctime)s %(levelname)s %(message)s",
        handlers=handlers,
    )


def read_token(env_file: str) -> str | None:
    """GITHUB_TOKEN from the environment, then a KEY=VALUE env file."""
    token = os.environ.get("GITHUB_TOKEN")
    if token:
        return token.strip()
    try:
        with open(env_file, encoding="utf-8") as fh:
            for line in fh:
                line = line.strip()
                if line.startswith("GITHUB_TOKEN="):
                    return line.split("=", 1)[1].strip().strip('"').strip("'")
    except FileNotFoundError:
        pass
    return None


class GitHub:
    def __init__(self, repo: str, token: str | None):
        self.api = f"https://api.github.com/repos/{repo}"
        self.headers = {
            "Accept": "application/vnd.github+json",
            "User-Agent": "fms-mirror-sync",
        }
        if token:
            self.headers["Authorization"] = f"Bearer {token}"

    def _open(self, url: str):
        return urllib.request.urlopen(urllib.request.Request(url, headers=self.headers))

    def get_json(self, url: str) -> dict:
        with self._open(url) as resp:
            return json.load(resp)

    def latest_release(self) -> dict:
        return self.get_json(f"{self.api}/releases/latest")

    def fetch(self, url: str, dest_part: Path) -> tuple[int, str]:
        """Stream url to dest_part, returning (bytes, sha256) computed on the fly."""
        digest = hashlib.sha256()
        written = 0
        with self._open(url) as resp, open(dest_part, "wb") as out:
            while chunk := resp.read(DOWNLOAD_CHUNK):
                digest.update(chunk)
                out.write(chunk)
                written += len(chunk)
        return written, digest.hexdigest()


def platform_key(asset_name: str) -> str | None:
    """Mirror make-latest-json.mjs's spelling: which updater key an asset answers to."""
    if asset_name.endswith(".exe"):
        return "windows-x86_64"
    if asset_name.endswith(".AppImage"):
        return "linux-aarch64" if "_arm64_" in asset_name else "linux-x86_64"
    if asset_name.endswith(".dmg"):
        return "darwin-aarch64"
    if asset_name.endswith(".apk"):
        return "android-aarch64"
    return None


def rewrite_manifest(manifest: dict, tag: str, base_url: str, mirrored: set[str]) -> dict:
    """Point every platform url at the mirror; keep signatures byte-identical.

    Raises unless each advertised url names an asset that actually mirrored.
    """
    want_prefix = f"{base_url}/mirror/{tag}/"
    notes_url = f"{base_url}/#download"
    out = dict(manifest)
    out["notes"] = notes_url
    platforms = {}
    for key, entry in manifest.get("platforms", {}).items():
        name = GITHUB_DL_RE.sub("", entry["url"])
        if name not in mirrored:
            raise RuntimeError(f"manifest advertises {name} for {key}, but it did not mirror")
        platforms[key] = {"signature": entry["signature"], "url": want_prefix + name}
    out["platforms"] = platforms
    return out


def write_json_atomic(path: Path, payload: dict) -> None:
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    os.replace(tmp, path)


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as fh:
        while chunk := fh.read(DOWNLOAD_CHUNK):
            digest.update(chunk)
    return digest.hexdigest()


def prune_old_tags(root: Path) -> None:
    """Keep the newest KEEP_TAG_DIRS completed tag directories; drop the rest."""
    done = sorted(
        (p for p in root.iterdir() if p.is_dir() and (p / ".done").exists()),
        key=lambda p: p.stat().st_mtime,
        reverse=True,
    )
    for child in done[KEEP_TAG_DIRS:]:
        logging.info("retention: removing %s", child.name)
        shutil.rmtree(child)


def bump_state(root: Path, ok: bool, dry_run: bool) -> None:
    if dry_run:
        return
    path = root / ".sync_state.json"
    state = {"failures": 0 if ok else 1, "updated": datetime.now(timezone.utc).isoformat()}
    if not ok and path.exists():
        try:
            state["failures"] = int(json.loads(path.read_text()).get("failures", 0)) + 1
        except (ValueError, json.JSONDecodeError, OSError):
            pass
    try:
        write_json_atomic(path, state)
    except OSError:
        pass


def sync(args) -> bool:
    gh = GitHub(args.repo, read_token(args.token_file))
    root = Path(args.root)
    base = args.base_url.rstrip("/")

    try:
        release = gh.latest_release()
    except Exception as exc:  # noqa: BLE001 - cron must log, not stack-trace
        logging.error("could not read the release feed: %s", exc)
        return False

    if release.get("prerelease"):
        logging.info("latest release %s is a pre-release; nothing to mirror", release["tag_name"])
        return True
    tag = release["tag_name"]
    tag_dir = root / tag
    marker = tag_dir / ".done"

    wanted = [a for a in release.get("assets", []) if ASSET_RE.match(a["name"])]
    manifest_asset = next((a for a in release.get("assets", []) if a["name"] == "latest.json"), None)
    if manifest_asset is None:
        logging.warning("%s carries no latest.json; mirroring files only", tag)

    mirrored: set[str] = set()
    records: dict[str, dict] = {}

    if marker.exists() and all((tag_dir / a["name"]).exists() for a in wanted):
        logging.info("%s already mirrored (%s); refreshing manifests only", tag, marker.read_text().strip())
        for a in wanted:
            mirrored.add(a["name"])
            records[a["name"]] = {
                "name": a["name"],
                "platform": platform_key(a["name"]),
                "size": a["size"],
                "sha256": sha256_of(tag_dir / a["name"]),
            }
    else:
        if not args.dry_run:
            tag_dir.mkdir(parents=True, exist_ok=True)
        for asset in wanted:
            name, url, size = asset["name"], asset["browser_download_url"], asset["size"]
            if args.dry_run:
                logging.info("would download %s (%d bytes)", name, size)
                mirrored.add(name)
                records[name] = {"name": name, "platform": platform_key(name), "size": size, "sha256": "(dry-run)"}
                continue
            final = tag_dir / name
            part = final.with_name(final.name + ".part")
            try:
                written, digest = gh.fetch(url, part)
                if written != size:
                    raise RuntimeError(f"{name}: got {written} bytes, API says {size}")
                os.replace(part, final)
            except Exception as exc:  # noqa: BLE001
                part.unlink(missing_ok=True)
                logging.error("download failed: %s", exc)
                return False
            logging.info("mirrored %s (%d bytes, sha256 %s)", name, written, digest[:12])
            mirrored.add(name)
            records[name] = {"name": name, "platform": platform_key(name), "size": written, "sha256": digest}

    # The manifest last: a rewritten latest.json must never name a missing file.
    rewritten = None
    if manifest_asset is not None:
        try:
            if args.dry_run:
                manifest = gh.get_json(manifest_asset["browser_download_url"])
                rewritten = rewrite_manifest(manifest, tag, base, mirrored)
                logging.info("would rewrite manifest urls to %s/mirror/%s/", base, tag)
            else:
                _, digest = gh.fetch(manifest_asset["browser_download_url"], tag_dir / "latest.json.part")
                manifest = json.loads((tag_dir / "latest.json.part").read_text(encoding="utf-8"))
                rewritten = rewrite_manifest(manifest, tag, base, mirrored)
                os.replace(tag_dir / "latest.json.part", tag_dir / "latest.json")
                os.replace(tag_dir / "latest.json", root / "latest.json")
                logging.info("published %s/latest.json (source sha256 %s)", root, digest[:12])
        except Exception as exc:  # noqa: BLE001
            (tag_dir / "latest.json.part").unlink(missing_ok=True)
            logging.error("manifest rewrite failed: %s", exc)
            return False

    if rewritten is not None and not args.dry_run:
        write_json_atomic(
            root / "releases.json",
            {
                "tag": tag,
                "version": rewritten.get("version", tag.lstrip("v")),
                "published_at": release.get("published_at"),
                "notes_url": f"{base}/#download",
                "assets": [records[n] for n in sorted(records)],
            },
        )
        marker.write_text(f"{tag} {datetime.now(timezone.utc).isoformat()}\n", encoding="utf-8")
    elif args.dry_run:
        logging.info("would publish releases.json for %s", tag)

    # Loud on the silent failure modes: a mirror older than the release, or a
    # run that kept failing — fallback users would otherwise sit on "up to date".
    published_at = release.get("published_at")
    if published_at and not args.dry_run:
        age = (datetime.now(timezone.utc) - datetime.strptime(published_at, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)).total_seconds()
        if age > STALE_AFTER:
            logging.warning("MIRROR_STALE: %s has been published %.0fh ago; this run should have covered it", tag, age / 3600)
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repo", default=os.environ.get("MIRROR_REPO", DEFAULT_REPO))
    parser.add_argument("--root", default=os.environ.get("FMS_MIRROR_DIR", DEFAULT_ROOT))
    parser.add_argument("--base-url", default=os.environ.get("MIRROR_BASE_URL", DEFAULT_BASE_URL))
    parser.add_argument("--token-file", default=DEFAULT_TOKEN_FILE)
    parser.add_argument("--log-file", default=DEFAULT_LOG_FILE)
    parser.add_argument("--dry-run", action="store_true", help="resolve and report; write nothing")
    parser.add_argument("-v", "--verbose", action="store_true")
    args = parser.parse_args()

    log_setup(args.log_file, args.verbose)
    if not args.dry_run:
        Path(args.root).mkdir(parents=True, exist_ok=True)

    ok = sync(args)
    if ok:
        bump_state(Path(args.root), ok=True, dry_run=args.dry_run)
        if not args.dry_run:
            prune_old_tags(Path(args.root))
        logging.info("mirror sync finished (dry_run=%s)", args.dry_run)
        return 0
    bump_state(Path(args.root), ok=False, dry_run=args.dry_run)
    try:
        failures = int(json.loads((Path(args.root) / ".sync_state.json").read_text()).get("failures", 0))
        if failures >= FAIL_ALERT_AT:
            logging.error("MIRROR_FAILING: %d consecutive sync failures", failures)
    except (OSError, ValueError, json.JSONDecodeError):
        pass
    return 1


if __name__ == "__main__":
    sys.exit(main())
