/** Small helpers shared across the "Read a book" feature. */

/** Generate a UUID (crypto.randomUUID with a fallback). */
export function getUUID(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  // Fallback: RFC4122-ish v4
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    const v = c === "x" ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
}

/** Current time as an RFC3339 string (matches the Rust backend format). */
export function nowIso(): string {
  return new Date().toISOString();
}

/** Highlight background colors for sentences. */
export const BG_COLORS: { key: string; label: string; swatch: string; bg: string }[] = [
  { key: "yellow", label: "Yellow", swatch: "bg-yellow-300", bg: "bg-yellow-200/70" },
  { key: "green", label: "Green", swatch: "bg-green-300", bg: "bg-green-200/70" },
  { key: "blue", label: "Blue", swatch: "bg-blue-300", bg: "bg-blue-200/70" },
  { key: "pink", label: "Pink", swatch: "bg-pink-300", bg: "bg-pink-200/70" },
  { key: "orange", label: "Orange", swatch: "bg-orange-300", bg: "bg-orange-200/70" },
];

/** Map a stored bg_color key to a Tailwind background class. */
export function sentenceBgClass(bgColor: string | null | undefined): string {
  if (!bgColor) return "";
  return BG_COLORS.find((c) => c.key === bgColor)?.bg ?? "";
}
