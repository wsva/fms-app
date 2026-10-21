"use client";

// Download section for the marketing site.
//
// The latest release is fetched client-side from the GitHub API — the app is a
// static export, so there is no build-time server to embed release metadata.
// Windows and Linux are the supported targets; Android ships but is flagged
// experimental. macOS is not offered (no button) but is reachable via
// "View all releases". The detected platform's card is highlighted.

import { useEffect, useMemo, useState } from "react";
import { Monitor, Terminal, Smartphone, Download, ExternalLink } from "lucide-react";
import { useLocale, type DictKey } from "@/lib/i18n";

const REPO = "wsva/fms-app";
const RELEASES_URL = `https://github.com/${REPO}/releases`;
const LATEST_API = `https://api.github.com/repos/${REPO}/releases/latest`;

interface GitHubAsset {
  name: string;
  browser_download_url: string;
  size: number;
}

interface GitHubRelease {
  tag_name: string;
  name: string | null;
  html_url: string;
  assets: GitHubAsset[];
}

type Platform = "windows" | "linux" | "android";

// Extension/keyword preference per platform, highest priority first. CI renames
// every asset to `FmS_<platform>_<arch>_<tag>.<ext>` (see
// .github/workflows/build.yml), but the lowercased-extension test also keeps
// working for the releases published before that rename, where Tauri's own names
// (`fms-app_0.1.0_x64-setup.exe`, `fms-app_0.1.0_amd64.deb`) are still attached.
const PLATFORM_MATCHERS: Record<Platform, ((name: string) => boolean)[]> = {
  windows: [(n) => n.endsWith(".exe"), (n) => n.endsWith(".msi")],
  linux: [
    (n) => n.endsWith(".appimage"),
    (n) => n.endsWith(".deb"),
    (n) => n.endsWith(".rpm"),
    (n) => n.endsWith(".tar.gz") && !n.includes("apple") && !n.includes("macos"),
  ],
  // A release built without the signing secrets carries only an `-unsigned` APK,
  // which Android refuses to install - prefer a signed one whenever both exist.
  android: [
    (n) => n.endsWith(".apk") && !n.includes("unsigned"),
    (n) => n.endsWith(".apk"),
    (n) => n.endsWith(".aab"),
  ],
};

function pickAsset(assets: GitHubAsset[], platform: Platform): GitHubAsset | null {
  const names = assets.map((a) => a.name.toLowerCase());
  for (const matches of PLATFORM_MATCHERS[platform]) {
    const idx = names.findIndex(matches);
    if (idx !== -1) return assets[idx];
  }
  return null;
}

function detectPlatform(): Platform | null {
  if (typeof navigator === "undefined") return null;
  const ua = navigator.userAgent;
  if (/android/i.test(ua)) return "android";
  if (/win/i.test(ua)) return "windows";
  if (/linux/i.test(ua)) return "linux";
  return null;
}

export default function DownloadSection({ id = "download" }: { id?: string }) {
  const { t } = useLocale();
  const [release, setRelease] = useState<GitHubRelease | null>(null);
  const [failed, setFailed] = useState(false);
  const [detected, setDetected] = useState<Platform | null>(null);

  useEffect(() => {
    setDetected(detectPlatform());
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 10_000);
    fetch(LATEST_API, {
      signal: controller.signal,
      headers: { Accept: "application/vnd.github+json" },
    })
      .then((res) => (res.ok ? res.json() : Promise.reject(new Error(String(res.status)))))
      .then((data: GitHubRelease) => setRelease(data))
      .catch(() => setFailed(true))
      .finally(() => clearTimeout(timer));
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, []);

  const version = release?.name || release?.tag_name || null;
  const picks = useMemo(() => {
    if (!release) return null;
    return {
      windows: pickAsset(release.assets, "windows"),
      linux: pickAsset(release.assets, "linux"),
      android: pickAsset(release.assets, "android"),
    };
  }, [release]);

  const cards: {
    platform: Platform;
    labelKey: DictKey;
    icon: typeof Monitor;
    experimental?: boolean;
  }[] = [
    { platform: "windows", labelKey: "download.windows", icon: Monitor },
    { platform: "linux", labelKey: "download.linux", icon: Terminal },
    { platform: "android", labelKey: "download.android", icon: Smartphone, experimental: true },
  ];

  return (
    <section id={id} className="scroll-mt-20 px-6 py-20 bg-bg-body">
      <div className="mx-auto max-w-5xl">
        <h2 className="text-3xl font-bold text-text-primary sm:text-4xl">{t("download.heading")}</h2>
        <p className="mt-3 max-w-2xl text-text-secondary">{t("download.sub")}</p>

        <div className="mt-6 flex flex-wrap items-center gap-x-4 gap-y-2 text-sm text-text-tertiary">
          {version ? (
            <span>
              {t("download.version")}:{" "}
              <span className="font-mono text-text-secondary">{version}</span>
            </span>
          ) : failed ? (
            <span>{t("download.unavailable")}</span>
          ) : (
            <span>{t("download.loading")}</span>
          )}
          {detected && <span className="text-accent">{t("download.detectHint")}</span>}
        </div>

        <div className="mt-8 grid gap-4 sm:grid-cols-3">
          {cards.map(({ platform, labelKey, icon: Icon, experimental }) => {
            const asset = picks?.[platform] ?? null;
            const href = asset?.browser_download_url ?? RELEASES_URL;
            const isDetected = detected === platform;
            const disabled = !asset && !!release; // release loaded but no asset for this OS
            return (
              <a
                key={platform}
                href={href}
                target="_blank"
                rel="noopener noreferrer"
                aria-disabled={disabled || undefined}
                className={`group flex flex-col rounded-xl border p-5 transition-colors ${
                  isDetected
                    ? "border-accent bg-accent-bg"
                    : "border-border-default bg-bg-card hover:border-accent"
                } ${disabled ? "opacity-50" : ""}`}
              >
                <div className="flex items-center justify-between">
                  <span className="flex items-center gap-2 font-semibold text-text-primary">
                    <Icon className="h-5 w-5 text-accent" />
                    {t(labelKey)}
                  </span>
                  {experimental && (
                    <span className="rounded-full bg-warning-bg px-2 py-0.5 text-xs font-medium text-warning-text">
                      {t("download.experimental")}
                    </span>
                  )}
                </div>
                <span className="mt-3 flex items-center gap-1.5 text-sm text-text-secondary">
                  {asset ? (
                    <>
                      <Download className="h-4 w-4" />
                      {asset.name}
                    </>
                  ) : (
                    <>
                      <ExternalLink className="h-4 w-4" />
                      {t("download.viewAll")}
                    </>
                  )}
                </span>
              </a>
            );
          })}
        </div>

        <a
          href={release?.html_url ?? RELEASES_URL}
          target="_blank"
          rel="noopener noreferrer"
          className="mt-6 inline-flex items-center gap-1.5 text-sm text-accent hover:underline"
        >
          <ExternalLink className="h-4 w-4" />
          {t("download.viewAll")}
        </a>
      </div>
    </section>
  );
}
