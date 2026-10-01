/**
 * Shared constants and types for AI word generation feature.
 * Used by both SentenceDrawer (single sentence) and BookAdvanced (bulk generation).
 */

// ---------------------------------------------------------------------------
// LocalStorage keys
// ---------------------------------------------------------------------------

export const LS_WORD_GEN_MODEL = "read_word_gen_model";
export const LS_WORD_GEN_PROMPT = "read_word_gen_prompt";
export const LS_WORD_GEN_TEMP = "read_word_gen_temp";
export const LS_WORD_GEN_LANG = "read_word_gen_lang";

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

export const DEFAULT_WORD_GEN_MODEL = "gemma4:e4b";
export const DEFAULT_WORD_GEN_LANG = "de";

// ---------------------------------------------------------------------------
// Language-specific prompts
// ---------------------------------------------------------------------------

export type WordGenLang = "de" | "en";

export const WORD_GEN_LANGUAGES: { value: WordGenLang; label: string }[] = [
  { value: "de", label: "German" },
  { value: "en", label: "English" },
];

export const WORD_GEN_PROMPTS: Record<WordGenLang, string> = {
  de: `You are a German language NLP assistant. Analyze the given German sentence and extract all meaningful words. For each word provide: the lemma (base/dictionary form), the part of speech, and whether it carries useful semantic meaning.
Rules:
- Restore separable verb parts to their base infinitive (e.g. 'steht auf' → 'aufstehen').
- Restore perfect/pluperfect forms to the base infinitive (e.g. 'ist gegangen' → 'gehen').
- Restore adjective declensions to the masculine nominative base form (e.g. 'guten' → 'gut').
- For nouns (Nomen), always include the definite article (der/die/das) in the lemma, e.g. 'Hunde' → 'der Hund', 'Katze' → 'die Katze', 'Haus' → 'das Haus'.
- Set keep=true for nouns, verbs, adjectives, adverbs with real meaning.
- Set keep=false for articles, prepositions, conjunctions, pronouns, auxiliary verbs, and other function words.
- Set keep=false for proper names of people (especially historical figures), places, and organizations — these are not vocabulary words to learn.
Respond ONLY with a JSON array, no other text. Each element: {"surface":"...","lemma":"...","pos":"...","keep":true/false}`,

  en: `You are an English language NLP assistant. Analyze the given English sentence and extract all meaningful words. For each word provide: the lemma (base/dictionary form), the part of speech, and whether it carries useful semantic meaning.
Rules:
- Restore inflected forms to their base form (e.g. 'running' → 'run', 'went' → 'go', 'better' → 'good').
- For nouns, use the singular form (e.g. 'dogs' → 'dog', 'children' → 'child').
- Set keep=true for nouns, verbs, adjectives, adverbs with real meaning.
- Set keep=false for articles (a, an, the), prepositions, conjunctions, pronouns, auxiliary verbs, and other function words.
- Set keep=false for proper names of people, places, and organizations — these are not vocabulary words to learn.
Respond ONLY with a JSON array, no other text. Each element: {"surface":"...","lemma":"...","pos":"...","keep":true/false}`,
};

/** Get the default prompt for a language. */
export function getDefaultPrompt(lang: WordGenLang): string {
  return WORD_GEN_PROMPTS[lang];
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** A single word item parsed from LLM response. */
export interface WordGenItem {
  surface: string;
  lemma: string;
  pos: string;
  keep: boolean;
}

/** Settings for word generation (persisted to localStorage). */
export interface WordGenSettings {
  model: string;
  prompt: string;
  temperature: number;
  lang: WordGenLang;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Load word generation settings from localStorage, falling back to defaults. */
export function loadWordGenSettings(): WordGenSettings {
  const lang = (localStorage.getItem(LS_WORD_GEN_LANG) || DEFAULT_WORD_GEN_LANG) as WordGenLang;
  return {
    model: localStorage.getItem(LS_WORD_GEN_MODEL) || DEFAULT_WORD_GEN_MODEL,
    prompt: localStorage.getItem(LS_WORD_GEN_PROMPT) || getDefaultPrompt(lang),
    temperature: parseFloat(localStorage.getItem(LS_WORD_GEN_TEMP) || "0") || 0,
    lang,
  };
}

/** Save a single setting to localStorage. */
export function saveWordGenSetting<K extends keyof WordGenSettings>(
  key: K,
  value: WordGenSettings[K]
): void {
  const lsKey =
    key === "model"
      ? LS_WORD_GEN_MODEL
      : key === "prompt"
      ? LS_WORD_GEN_PROMPT
      : key === "lang"
      ? LS_WORD_GEN_LANG
      : LS_WORD_GEN_TEMP;
  localStorage.setItem(lsKey, String(value));
}

/**
 * Parse the LLM response and extract the JSON array.
 * Returns null if parsing fails.
 */
export function parseWordGenResponse(raw: string): WordGenItem[] | null {
  const jsonMatch = raw.match(/\[[\s\S]*\]/);
  if (!jsonMatch) return null;
  try {
    return JSON.parse(jsonMatch[0]);
  } catch {
    return null;
  }
}

/** Temperature options for the selector. */
export const TEMPERATURE_OPTIONS = [0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0];
