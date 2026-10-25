// Tailwind fragments shared by the Agent page panels. Kept in one place so the
// three tabs and the approval card cannot drift apart stylistically.

export const btnGhost =
  "px-3 py-1.5 text-xs rounded-md border border-border-default hover:bg-bg-hover cursor-pointer disabled:cursor-not-allowed disabled:opacity-50";

export const inputCls =
  "w-full px-3 py-2 border border-border-default rounded-md bg-bg-input text-text-primary";

export const codeBox =
  "px-3 py-2 border border-border-default rounded-md bg-bg-input text-text-primary text-xs break-all";

export const panelTitle = "text-[1.05em] font-semibold text-text-primary";

// The page body is full width, but prose is not: a hint that runs the width of a
// 1600px window is unreadable, while the code payloads beside it are exactly what
// wants the room. The cap lives here so every paragraph in every panel obeys it.
export const hint = "max-w-3xl text-text-secondary text-sm";
