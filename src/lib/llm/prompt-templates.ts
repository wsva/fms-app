/**
 * Prompt templates for LLM Chat prompt builder.
 * Each template has localized versions.
 * The {content} placeholder is replaced with user's input.
 */

export type PromptLanguage = "en" | "de";

export interface PromptTemplate {
  id: string;
  name: Record<PromptLanguage, string>;
  prompt: Record<PromptLanguage, string>;
  /**
   * Optional explicit system-prompt instruction. When omitted, the system
   * prompt is derived from `prompt` by removing the `{content}` slot (see
   * {@link buildSystemPrompt}).
   */
  system?: Partial<Record<PromptLanguage, string>>;
}

export const PROMPT_LANGUAGES: { value: PromptLanguage; label: string }[] = [
  { value: "en", label: "English" },
  { value: "de", label: "Deutsch" },
];

export const PROMPT_TEMPLATES: PromptTemplate[] = [
  {
    id: "explain_word",
    name: {
      en: "Explain Word",
      de: "Wort erklären",
    },
    prompt: {
      en: "Explain the following word in detail, including its meaning, usage, and example sentences:\n\n{content}",
      de: "Erkläre das folgende Wort im Detail, einschließlich seiner Bedeutung, Verwendung und Beispielsätze:\n\n{content}",
    },
  },
  {
    id: "explain_content",
    name: {
      en: "Explain Content",
      de: "Inhalt erklären",
    },
    prompt: {
      en: "Explain the following content in clear and simple terms:\n\n{content}",
      de: "Erkläre den folgenden Inhalt in klaren und einfachen Worten:\n\n{content}",
    },
  },
  {
    id: "analyze_sentence",
    name: {
      en: "Analyze Sentence",
      de: "Satz analysieren",
    },
    prompt: {
      en: "Analyze the sentence structure of the following text. Break down the grammar, identify clauses, and explain the syntax:\n\n{content}",
      de: "Analysiere die Satzstruktur des folgenden Textes. Zerlege die Grammatik, identifiziere Nebensätze und erkläre die Syntax:\n\n{content}",
    },
  },
  {
    id: "translate",
    name: {
      en: "Translate",
      de: "Übersetzen",
    },
    prompt: {
      en: "Translate the following text to German:\n\n{content}",
      de: "Übersetze den folgenden Text ins Englische:\n\n{content}",
    },
  },
  {
    id: "synonyms",
    name: {
      en: "Find Synonyms",
      de: "Synonyme finden",
    },
    prompt: {
      en: "List synonyms and related words for the following term, with brief explanations of nuances:\n\n{content}",
      de: "Liste Synonyme und verwandte Wörter für den folgenden Begriff auf, mit kurzen Erklärungen der Nuancen:\n\n{content}",
    },
  },
  {
    id: "simplify",
    name: {
      en: "Simplify",
      de: "Vereinfachen",
    },
    prompt: {
      en: "Rewrite the following text in simpler, easier-to-understand language:\n\n{content}",
      de: "Schreibe den folgenden Text in einfacherer, leichter verständlicher Sprache um:\n\n{content}",
    },
  },
  {
    id: "summarize",
    name: {
      en: "Summarize",
      de: "Zusammenfassen",
    },
    prompt: {
      en: "Provide a concise summary of the following text:\n\n{content}",
      de: "Erstelle eine kurze Zusammenfassung des folgenden Textes:\n\n{content}",
    },
  },
];

/**
 * Build a prompt by filling the template with content.
 */
export function buildPrompt(
  templateId: string,
  language: PromptLanguage,
  content: string
): string | null {
  const template = PROMPT_TEMPLATES.find((t) => t.id === templateId);
  if (!template) return null;
  return template.prompt[language].replace("{content}", content);
}

/**
 * Build the system-prompt instruction for a template.
 *
 * The chat page maps a selected template onto the model's *system prompt*
 * (goose-sdk takes the system prompt separately from the conversation), so the
 * user's own text stays the user message. Uses the template's explicit `system`
 * override when present; otherwise derives it from `prompt` by dropping the
 * `{content}` slot and any trailing separator/whitespace.
 */
export function buildSystemPrompt(
  templateId: string,
  language: PromptLanguage
): string | null {
  const template = PROMPT_TEMPLATES.find((t) => t.id === templateId);
  if (!template) return null;
  const explicit = template.system?.[language];
  if (explicit && explicit.trim()) return explicit.trim();
  return template.prompt[language]
    .replace("{content}", "")
    .replace(/[\s:]+$/, "")
    .trim();
}
