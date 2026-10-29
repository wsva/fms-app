"use client";

// Client-side i18n for the marketing site.
//
// The app is a Next.js *static export* (`output: 'export'` in next.config.mjs),
// so there is no server runtime: cookie/SSR-based locale negotiation and the
// built-in `i18n` routing are both unavailable. The locale is therefore resolved
// entirely on the client — seeded from `localStorage`, falling back to the
// browser language — and the first (prerendered) paint always renders English so
// server HTML and client hydration match. Components that show the chosen
// locale should read it after mount (the provider flips `ready` once it has).

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";

export type Locale = "en" | "de";

export const LOCALES: { code: Locale; label: string }[] = [
  { code: "en", label: "English" },
  { code: "de", label: "Deutsch" },
];

const STORAGE_KEY = "fms-locale";

const dict = {
  en: {
    "brand.name": "FmS",
    "brand.tagline": "Fluent by ear",
    "nav.features": "Features",
    "nav.how": "How it works",
    "nav.download": "Download",
    "nav.privacy": "Privacy",
    "nav.openApp": "Open app",
    "nav.language": "Language",

    "hero.badge": "Free & open source · Windows · Linux",
    "hero.title": "Learn a language by listening, not by scrolling",
    "hero.subtitle":
      "FmS turns any audio into dictation practice. Import a dataset, generate subtitles locally with on-device speech recognition, and train your ear one cue at a time — with spaced-repetition cards, read-aloud scoring and cross-device sync.",
    "hero.ctaDownload": "Download for free",
    "hero.ctaOpenApp": "Open the app",
    "hero.note": "No account required. Your audio and progress never leave your device.",

    "features.heading": "Everything you need to train your ear",
    "features.sub": "A desktop-first toolkit built around real listening practice.",
    "features.dictation.title": "Dictation practice",
    "features.dictation.desc":
      "Transcribe what you hear against waveform-synced cues, with instant diffing and per-cue replay.",
    "features.cards.title": "Spaced repetition",
    "features.cards.desc":
      "Turn phrases into flashcards and review them with an SM-2 scheduler and full-text search.",
    "features.readaloud.title": "Read-aloud scoring",
    "features.readaloud.desc":
      "Record yourself reading a text and get a similarity score from on-device speech recognition.",
    "features.stt.title": "Local speech recognition",
    "features.stt.desc":
      "Generate subtitles with bundled Parakeet models — everything runs offline on your machine.",
    "features.sync.title": "Cross-device sync",
    "features.sync.desc":
      "Pair your phone and PC over the local network or a hub and keep progress in sync.",
    "features.agent.title": "Agent friendly",
    "features.agent.desc":
      "Every feature is exposed through a built-in MCP server, so AI agents can drive the app too.",

    "how.heading": "How it works",
    "how.sub": "From raw audio to daily practice in four steps.",
    "how.step1.title": "Import a dataset",
    "how.step1.desc": "Drop in audio or video files plus optional reference text.",
    "how.step2.title": "Generate subtitles",
    "how.step2.desc": "Local STT transcribes each track and aligns it into timed cues.",
    "how.step3.title": "Practice dictation",
    "how.step3.desc": "Listen, type what you hear, and compare against the cue.",
    "how.step4.title": "Review & track",
    "how.step4.desc": "Save cards, schedule reviews and watch your streak grow.",

    "download.heading": "Download FmS",
    "download.sub": "Grab the latest release for your platform. It's free, and the source is open.",
    "download.windows": "Windows",
    "download.linux": "Linux",
    "download.android": "Android",
    "download.experimental": "Experimental",
    "download.loading": "Checking for the latest release…",
    "download.unavailable":
      "Couldn't reach GitHub right now. You can still browse all releases manually.",
    "download.viewAll": "View all releases",
    "download.version": "Latest version",
    "download.detectHint": "Detected your platform — the recommended build is highlighted.",

    "privacy.heading": "Private by design",
    "privacy.p1":
      "FmS is local-first. Audio, generated subtitles, waveforms and your progress are stored on your own device — nothing is uploaded to a server to be processed.",
    "privacy.p2":
      "Speech recognition runs entirely offline using bundled models. There is no telemetry, no analytics and no third-party tracking in the app.",
    "privacy.p3":
      "Cross-device sync is opt-in and travels over your own network or a hub you control. The source code is open so you can verify all of this yourself.",

    "footer.rights": "All rights reserved.",
    "footer.github": "GitHub",
    "footer.privacy": "Privacy",
    "footer.builtWith": "Built with Tauri, Next.js and Rust.",
  },
  de: {
    "brand.name": "FmS",
    "brand.tagline": "Fluent übers Ohr",
    "nav.features": "Funktionen",
    "nav.how": "So funktioniert's",
    "nav.download": "Download",
    "nav.privacy": "Datenschutz",
    "nav.openApp": "App öffnen",
    "nav.language": "Sprache",

    "hero.badge": "Kostenlos & Open Source · Windows · Linux",
    "hero.title": "Eine Sprache durch Hören lernen — nicht durch Scrollen",
    "hero.subtitle":
      "FmS verwandelt jedes Audio in Diktat-Training. Importiere ein Dataset, erzeuge Untertitel lokal mit On-Device-Spracherkennung und trainiere dein Gehör Cue für Cue — mit Spaced-Repetition-Karten, Vorlese-Bewertung und geräteübergreifender Synchronisation.",
    "hero.ctaDownload": "Kostenlos herunterladen",
    "hero.ctaOpenApp": "App öffnen",
    "hero.note": "Kein Konto nötig. Deine Audiodaten und dein Fortschritt bleiben auf deinem Gerät.",

    "features.heading": "Alles, was du fürs Hörtraining brauchst",
    "features.sub": "Ein Desktop-First-Werkzeugkasten rund um echtes Hörverstehen.",
    "features.dictation.title": "Diktat-Training",
    "features.dictation.desc":
      "Transkribiere Gehörtes gegen wellenform-synchronisierte Cues — mit sofortigem Vergleich und Cue-Wiederholung.",
    "features.cards.title": "Spaced Repetition",
    "features.cards.desc":
      "Verwandle Wendungen in Karteikarten und wiederhole sie mit SM-2-Planer und Volltextsuche.",
    "features.readaloud.title": "Vorlese-Bewertung",
    "features.readaloud.desc":
      "Nimm dich beim Vorlesen auf und erhalte eine Ähnlichkeitsbewertung aus der On-Device-Spracherkennung.",
    "features.stt.title": "Lokale Spracherkennung",
    "features.stt.desc":
      "Erzeuge Untertitel mit gebündelten Parakeet-Modellen — alles läuft offline auf deinem Rechner.",
    "features.sync.title": "Geräteübergreifende Sync",
    "features.sync.desc":
      "Kopple Telefon und PC im lokalen Netz oder über einen Hub und halte den Fortschritt synchron.",
    "features.agent.title": "Agenten-freundlich",
    "features.agent.desc":
      "Jede Funktion ist über einen eingebauten MCP-Server erreichbar, sodass auch KI-Agenten die App steuern können.",

    "how.heading": "So funktioniert's",
    "how.sub": "In vier Schritten vom rohen Audio zur täglichen Übung.",
    "how.step1.title": "Dataset importieren",
    "how.step1.desc": "Audio- oder Videodateien plus optionalen Referenztext hinzufügen.",
    "how.step2.title": "Untertitel erzeugen",
    "how.step2.desc": "Lokale STT transkribiert jeden Titel und richtet ihn in zeitliche Cues aus.",
    "how.step3.title": "Diktat üben",
    "how.step3.desc": "Hören, tippen was du hörst und mit dem Cue vergleichen.",
    "how.step4.title": "Wiederholen & verfolgen",
    "how.step4.desc": "Karten speichern, Wiederholungen planen und deinen Streak wachsen sehen.",

    "download.heading": "FmS herunterladen",
    "download.sub": "Hole dir die neueste Version für deine Plattform. Kostenlos und quelloffen.",
    "download.windows": "Windows",
    "download.linux": "Linux",
    "download.android": "Android",
    "download.experimental": "Experimentell",
    "download.loading": "Suche die neueste Version…",
    "download.unavailable":
      "GitHub ist gerade nicht erreichbar. Du kannst alle Releases dennoch manuell durchsehen.",
    "download.viewAll": "Alle Releases anzeigen",
    "download.version": "Neueste Version",
    "download.detectHint": "Plattform erkannt — der empfohlene Build ist hervorgehoben.",

    "privacy.heading": "Privat by design",
    "privacy.p1":
      "FmS ist local-first. Audio, erzeugte Untertitel, Wellenformen und dein Fortschritt werden auf deinem eigenen Gerät gespeichert — nichts wird zur Verarbeitung auf einen Server geladen.",
    "privacy.p2":
      "Spracherkennung läuft vollständig offline mit gebündelten Modellen. Es gibt keine Telemetrie, keine Analytics und kein Tracking durch Dritte in der App.",
    "privacy.p3":
      "Geräteübergreifende Sync ist opt-in und läuft über dein eigenes Netz oder einen Hub, den du kontrollierst. Der Quellcode ist offen, damit du all das selbst prüfen kannst.",

    "footer.rights": "Alle Rechte vorbehalten.",
    "footer.github": "GitHub",
    "footer.privacy": "Datenschutz",
    "footer.builtWith": "Gebaut mit Tauri, Next.js und Rust.",
  },
} as const satisfies Record<string, Record<string, string>>;

export type DictKey = keyof (typeof dict)["en"];

interface LocaleContextValue {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  t: (key: DictKey) => string;
  /** False until the stored/browser locale has been resolved on the client. */
  ready: boolean;
}

const LocaleContext = createContext<LocaleContextValue | null>(null);

export function LocaleProvider({ children }: { children: ReactNode }) {
  // Always start as English so the prerendered HTML and the first client render
  // agree; the effect below swaps in the real locale after mount.
  const [locale, setLocaleState] = useState<Locale>("en");
  const [ready, setReady] = useState(false);

  useEffect(() => {
    let initial: Locale = "en";
    try {
      const stored = localStorage.getItem(STORAGE_KEY);
      if (stored === "en" || stored === "de") {
        initial = stored;
      } else if (navigator.language?.toLowerCase().startsWith("de")) {
        initial = "de";
      }
    } catch {
      /* localStorage / navigator unavailable — keep the English default */
    }
    setLocaleState(initial);
    setReady(true);
  }, []);

  useEffect(() => {
    if (ready) document.documentElement.lang = locale;
  }, [ready, locale]);

  const setLocale = useCallback((next: Locale) => {
    setLocaleState(next);
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      /* ignore persistence failures (private mode, quota) */
    }
  }, []);

  const t = useCallback(
    (key: DictKey) => dict[locale][key] ?? dict.en[key],
    [locale],
  );

  return (
    <LocaleContext.Provider value={{ locale, setLocale, t, ready }}>
      {children}
    </LocaleContext.Provider>
  );
}

export function useLocale(): LocaleContextValue {
  const ctx = useContext(LocaleContext);
  if (!ctx) throw new Error("useLocale must be used within a LocaleProvider");
  return ctx;
}
