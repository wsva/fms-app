"use client";

// The public marketing landing page served at `/`.
//
// Inside the Tauri desktop/mobile app the window loads `/` from the static
// export, so the very first thing this page does is bounce to `/home` (the real
// app shell) and render only a splash until that navigation commits — otherwise
// the packaged app would flash the website. On the web, `/` always shows this
// page; the "Open app" buttons link to `/home`.

import { useEffect, useLayoutEffect, useState } from "react";
import { useRouter } from "next/navigation";
import {
  Headphones,
  BookOpen,
  Layers,
  Mic,
  NotebookPen,
  Bot,
  Code2,
  ArrowRight,
  ShieldCheck,
} from "lucide-react";
import { isTauri } from "@/lib/platform";
import { useLocale, LOCALES, type DictKey } from "@/lib/i18n";
import DownloadSection from "./DownloadSection";

const REPO_URL = "https://github.com/wsva/fms-app";
// The user manual lives in the repo under docs/manual; each feature card links
// to the page that explains it in depth.
const MANUAL_URL = `${REPO_URL}/blob/main/docs/manual`;

// useLayoutEffect warns during server prerender, so fall back to useEffect
// there and use the layout effect only in the browser (where it runs after
// commit but before paint — exactly what the Tauri splash swap below needs).
const useIsomorphicLayoutEffect =
  typeof window !== "undefined" ? useLayoutEffect : useEffect;

const FEATURES: {
  titleKey: DictKey;
  descKey: DictKey;
  icon: typeof Headphones;
  learn: string;
}[] = [
  { titleKey: "features.dictation.title", descKey: "features.dictation.desc", icon: Headphones, learn: "dictation.md" },
  { titleKey: "features.books.title", descKey: "features.books.desc", icon: BookOpen, learn: "books.md" },
  { titleKey: "features.readaloud.title", descKey: "features.readaloud.desc", icon: Mic, learn: "read_aloud.md" },
  { titleKey: "features.cards.title", descKey: "features.cards.desc", icon: Layers, learn: "cards.md" },
  { titleKey: "features.wiki.title", descKey: "features.wiki.desc", icon: NotebookPen, learn: "wiki.md" },
  { titleKey: "features.agent.title", descKey: "features.agent.desc", icon: Bot, learn: "agents.md" },
];

const STEPS: { titleKey: DictKey; descKey: DictKey }[] = [
  { titleKey: "how.step1.title", descKey: "how.step1.desc" },
  { titleKey: "how.step2.title", descKey: "how.step2.desc" },
  { titleKey: "how.step3.title", descKey: "how.step3.desc" },
  { titleKey: "how.step4.title", descKey: "how.step4.desc" },
];

function LanguageToggle() {
  const { locale, setLocale, t } = useLocale();
  return (
    <div
      className="flex items-center gap-1 rounded-full border border-border-default bg-bg-card p-0.5"
      role="group"
      aria-label={t("nav.language")}
    >
      {LOCALES.map(({ code, label }) => (
        <button
          key={code}
          type="button"
          onClick={() => setLocale(code)}
          aria-pressed={locale === code}
          className={`rounded-full px-2.5 py-1 text-xs font-medium transition-colors ${
            locale === code
              ? "bg-accent text-bg-body"
              : "text-text-secondary hover:text-text-primary"
          }`}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

export default function MarketingPage() {
  const router = useRouter();
  const { t } = useLocale();
  // The full marketing page is prerendered (so the static `/` HTML carries real
  // content for SEO and link previews). Inside the Tauri shell the window loads
  // `/`, so we bounce to `/home` before the browser paints: flipping `native` in
  // a layout effect re-renders synchronously after commit, swapping the
  // prerendered marketing for the splash without ever showing it.
  const [native, setNative] = useState(false);

  useIsomorphicLayoutEffect(() => {
    if (isTauri()) {
      setNative(true);
      router.replace("/home");
    }
  }, [router]);

  if (native) {
    return (
      <div className="flex min-h-screen flex-col items-center justify-center bg-bg-body">
        {/* /logo.png is a copy of the launcher artwork (see public/). */}
        <img
          src="/logo.png"
          alt=""
          className="h-28 w-28 select-none rounded-2xl"
          draggable={false}
        />
      </div>
    );
  }

  return (
    <div className="min-h-screen bg-bg-body text-text-primary">
      {/* Nav */}
      <header className="sticky top-0 z-40 border-b border-border-default bg-bg-body/80 backdrop-blur">
        <div className="mx-auto flex h-16 max-w-6xl items-center justify-between px-6">
          <a href="#top" className="flex items-center gap-2 font-semibold">
            <img src="/logo.png" alt="" className="h-7 w-7 rounded-lg" draggable={false} />
            <span>{t("brand.name")}</span>
            <span className="hidden text-sm font-normal text-text-tertiary sm:inline">
              {t("brand.tagline")}
            </span>
          </a>

          <nav className="hidden items-center gap-6 text-sm text-text-secondary md:flex">
            <a href="#features" className="hover:text-text-primary">
              {t("nav.features")}
            </a>
            <a href="#how" className="hover:text-text-primary">
              {t("nav.how")}
            </a>
            <a href="#download" className="hover:text-text-primary">
              {t("nav.download")}
            </a>
            <a href="#privacy" className="hover:text-text-primary">
              {t("nav.privacy")}
            </a>
          </nav>

          <div className="flex items-center gap-3">
            <LanguageToggle />
            <a
              href="/home"
              className="hidden items-center gap-1.5 rounded-lg bg-accent-hover px-3.5 py-2 text-sm font-medium text-bg-body transition-colors hover:bg-accent sm:inline-flex"
            >
              {t("nav.openApp")}
              <ArrowRight className="h-4 w-4" />
            </a>
          </div>
        </div>
      </header>

      <main id="top">
        {/* Hero */}
        <section className="px-6 py-24 sm:py-32">
          <div className="mx-auto max-w-3xl text-center">
            <span className="inline-flex items-center rounded-full border border-border-default bg-bg-card px-3 py-1 text-xs font-medium text-text-secondary">
              {t("hero.badge")}
            </span>
            <h1 className="mt-6 text-4xl font-extrabold leading-tight tracking-tight sm:text-6xl">
              {t("hero.title")}
            </h1>
            <p className="mt-6 text-lg text-text-secondary">{t("hero.subtitle")}</p>
            <div className="mt-8 flex flex-wrap items-center justify-center gap-3">
              <a
                href="#download"
                className="inline-flex items-center gap-2 rounded-lg bg-accent-hover px-5 py-3 font-medium text-bg-body transition-colors hover:bg-accent"
              >
                {t("hero.ctaDownload")}
                <ArrowRight className="h-4 w-4" />
              </a>
              <a
                href="/home"
                className="inline-flex items-center gap-2 rounded-lg border border-border-default bg-bg-card px-5 py-3 font-medium text-text-primary transition-colors hover:border-accent"
              >
                {t("hero.ctaOpenApp")}
              </a>
            </div>
            <p className="mt-6 text-sm text-text-tertiary">{t("hero.note")}</p>
            <p className="mt-3 inline-flex items-center gap-1.5 text-xs font-medium uppercase tracking-wide text-text-tertiary">
              <ShieldCheck className="h-4 w-4 text-accent" />
              {t("hero.techStrip")}
            </p>
          </div>
        </section>

        {/* Features */}
        <section id="features" className="scroll-mt-20 border-t border-border-default px-6 py-20">
          <div className="mx-auto max-w-6xl">
            <h2 className="text-3xl font-bold sm:text-4xl">{t("features.heading")}</h2>
            <p className="mt-3 max-w-2xl text-text-secondary">{t("features.sub")}</p>
            <div className="mt-10 grid gap-6 sm:grid-cols-2 lg:grid-cols-3">
              {FEATURES.map(({ titleKey, descKey, icon: Icon, learn }) => (
                <div
                  key={titleKey}
                  className="flex flex-col rounded-xl border border-border-default bg-bg-card p-6"
                >
                  <span className="inline-flex h-10 w-10 items-center justify-center rounded-lg bg-bg-muted text-accent">
                    <Icon className="h-5 w-5" />
                  </span>
                  <h3 className="mt-4 font-semibold">{t(titleKey)}</h3>
                  <p className="mt-2 text-sm text-text-secondary">{t(descKey)}</p>
                  <a
                    href={`${MANUAL_URL}/${learn}`}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="mt-4 inline-flex items-center gap-1 self-start text-sm font-medium text-accent hover:underline"
                  >
                    {t("features.link")}
                    <ArrowRight className="h-4 w-4" />
                  </a>
                </div>
              ))}
            </div>
          </div>
        </section>

        {/* How it works */}
        <section
          id="how"
          className="scroll-mt-20 border-t border-border-default bg-bg-card/40 px-6 py-20"
        >
          <div className="mx-auto max-w-5xl">
            <h2 className="text-3xl font-bold sm:text-4xl">{t("how.heading")}</h2>
            <p className="mt-3 max-w-2xl text-text-secondary">{t("how.sub")}</p>
            <ol className="mt-10 grid gap-6 sm:grid-cols-2 lg:grid-cols-4">
              {STEPS.map(({ titleKey, descKey }, i) => (
                <li key={titleKey} className="relative rounded-xl border border-border-default bg-bg-card p-6">
                  <span className="flex h-8 w-8 items-center justify-center rounded-full bg-accent text-sm font-bold text-bg-body">
                    {i + 1}
                  </span>
                  <h3 className="mt-4 font-semibold">{t(titleKey)}</h3>
                  <p className="mt-2 text-sm text-text-secondary">{t(descKey)}</p>
                </li>
              ))}
            </ol>
          </div>
        </section>

        {/* Download */}
        <div className="border-t border-border-default">
          <DownloadSection id="download" />
        </div>

        {/* Privacy */}
        <section id="privacy" className="scroll-mt-20 border-t border-border-default px-6 py-20">
          <div className="mx-auto max-w-3xl">
            <h2 className="flex items-center gap-2 text-3xl font-bold sm:text-4xl">
              <ShieldCheck className="h-8 w-8 text-accent" />
              {t("privacy.heading")}
            </h2>
            <div className="mt-6 space-y-4 text-text-secondary">
              <p>{t("privacy.p1")}</p>
              <p>{t("privacy.p2")}</p>
              <p>{t("privacy.p3")}</p>
            </div>
          </div>
        </section>
      </main>

      {/* Footer */}
      <footer className="border-t border-border-default px-6 py-10">
        <div className="mx-auto flex max-w-6xl flex-col items-center justify-between gap-4 text-sm text-text-tertiary sm:flex-row">
          <div className="flex items-center gap-2">
            <img src="/logo.png" alt="" className="h-6 w-6 rounded-md" draggable={false} />
            <span>
              © {new Date().getFullYear()} {t("brand.name")}. {t("footer.rights")}
            </span>
          </div>
          <div className="flex items-center gap-5">
            <a
              href={REPO_URL}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1.5 hover:text-text-primary"
            >
              <Code2 className="h-4 w-4" />
              {t("footer.github")}
            </a>
            <a href="#privacy" className="hover:text-text-primary">
              {t("footer.privacy")}
            </a>
          </div>
          <span className="text-center sm:text-right">{t("footer.builtWith")}</span>
        </div>
      </footer>
    </div>
  );
}
