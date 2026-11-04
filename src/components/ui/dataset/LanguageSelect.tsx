"use client";

import { useEffect, useMemo, useState } from "react";
import { listLanguages } from "@/lib/edge_tts/speak";
import { localeLabel } from "@/lib/edge_tts/voices";
import { isTauri } from "@/lib/tauri";

type Props = {
  /** The dataset's language tag (`""` = unset). Free-form tags a card back face
   *  already holds (e.g. `de-AT`) narrow to their primary subtag for the pick,
   *  the same way SpeakToolbar does — the backend's `normalize_language` accepts
   *  either shape, and a per-language voice preference resolves through the
   *  primary subtag when it has no exact-tag entry. */
  value: string;
  onChange: (language: string) => void;
  /** Label for the empty choice. */
  placeholder?: string;
  className?: string;
  disabled?: boolean;
  title?: string;
  "aria-label"?: string;
};

/**
 * A dropdown over the languages Edge TTS actually has voices for.
 *
 * The same list SpeakToolbar narrows the reading voice by; used here to decide
 * a dataset's own `language` field — a fact about the texts, not a voice pick —
 * so creating or editing a dataset never has to type a tag by hand and hope it
 * matches what a preference is keyed by.
 */
export default function LanguageSelect({
  value,
  onChange,
  placeholder = "No language",
  className,
  disabled,
  title,
  ...rest
}: Props) {
  const [langs, setLangs] = useState<string[]>([]);

  useEffect(() => {
    if (!isTauri()) return;
    let alive = true;
    listLanguages().then((l) => alive && setLangs(l));
    return () => {
      alive = false;
    };
  }, []);

  const options = useMemo(() => ["", ...langs], [langs]);

  const current = value.trim()
    ? (options.includes(value.trim()) ? value.trim() : value.trim().split("-")[0].toLowerCase())
    : "";

  return (
    <select
      className={className}
      value={current}
      onChange={(e) => onChange(e.target.value)}
      disabled={disabled}
      title={title}
      {...rest}
    >
      {options.map((code) => (
        <option key={code || "none"} value={code}>
          {code ? localeLabel(code) : placeholder}
        </option>
      ))}
    </select>
  );
}
