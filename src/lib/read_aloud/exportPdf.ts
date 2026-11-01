/**
 * Export a read-aloud dataset's texts to PDF.
 *
 * Why print and not a PDF library: the whole point of the export is the *font* —
 * the same German school script (Playwrite DE, a self-hosted variable woff2) the
 * reader is practising in. The PDF libraries available here cannot embed a
 * variable woff2 handwriting face, so anything they produce would silently fall
 * back to a plain typeface and miss the exercise's purpose. The webview can draw
 * that face exactly, so we build a print-only document carrying the app's own
 * @font-face rules and let the webview's built-in "Save as PDF" write the file.
 * The print dialog is scoped to a hidden iframe, so only the texts print — never
 * the surrounding app chrome.
 *
 * Every text starts on its own page (`break-before: page` on all but the first),
 * which is what makes the result usable as a set of practice sheets.
 */

import type { ReadText } from "@/lib/read_aloud/types";

/** The writing to typeset the texts in, mirroring the on-screen dials. */
export type RaPrintFont = {
  /** The `font-family` value — one of the Playwrite stacks, or empty for the UI
   *  font. Same string `raFontOption().css` hands the practice pane. */
  familyCss: string;
  /** The `word-spacing` addition — `""`, `0.3em` or `0.6em`. Same value the
   *  pane gets from `raSpaceOption().css`. */
  spaceCss: string;
};

/** The four German school scripts, declared here so the print document is
 *  self-contained: a fresh iframe has none of the app's stylesheets. The URLs
 *  are resolved against the live page so they reach `/fonts/...` on whatever
 *  origin the webview is served from (dev server or the packaged `tauri://`). */
const FONT_FACES: { family: string; file: string }[] = [
  { family: "Playwrite DE Grund", file: "playwrite-de-grund-wght.woff2" },
  { family: "Playwrite DE VA", file: "playwrite-de-va-wght.woff2" },
  { family: "Playwrite DE SAS", file: "playwrite-de-sas-wght.woff2" },
  { family: "Playwrite DE LA", file: "playwrite-de-la-wght.woff2" },
];

/** The interface stack, used when the current writing is the UI font (empty
 *  `familyCss`) and as the fallback tail of every script. Matches globals.css. */
const UI_STACK = "Inter, Avenir, Helvetica, Arial, sans-serif";

/** Which step of the export is running, and how far along it is (0–100). The
 *  caller paints this into an overlay; every step is also logged, so a silent
 *  or empty result can be traced to the exact stage in the Log page. */
export type RaExportProgress = { step: string; message: string; percent: number };

/** Prefix shared by every export log line — greppable in the console and in
 *  the app log once it is forwarded there. */
const LOG_TAG = "[exportPdf]";

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** The print-only HTML: one block per text, each on its own page. */
function buildDocument(datasetName: string, texts: ReadText[], font: RaPrintFont): string {
  const origin = typeof window !== "undefined" ? window.location.origin : "";
  const faces = FONT_FACES.map(
    (f) => `@font-face {
  font-family: "${f.family}";
  font-style: normal;
  font-weight: 100 400;
  font-display: block;
  src: url("${origin}/fonts/${f.file}") format("woff2");
}`
  ).join("\n");

  const family = font.familyCss || UI_STACK;

  const pages = texts
    .map((t) => {
      const title = escapeHtml(t.title || "Untitled");
      const note = t.note ? `<p class="note">${escapeHtml(t.note)}</p>` : "";
      const content = escapeHtml(t.content).replace(/\n/g, "<br/>");
      return `  <section class="page">
    <h1 class="title">${title}</h1>
${note}    <div class="content">${content}</div>
  </section>`;
    })
    .join("\n");

  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8" />
<title>${escapeHtml(datasetName)}</title>
<style>
${faces}
@page { size: A4; margin: 18mm 16mm; }
* { box-sizing: border-box; }
html, body { margin: 0; padding: 0; color: #111; background: #fff; }
body { font-family: ${UI_STACK}; font-size: 16px; line-height: 1.5; }
/* The one line the script and the spacing apply to — the text being read, never
   its title or note, which stay in the interface font. */
.content {
  font-family: ${family};
  word-spacing: ${font.spaceCss || "normal"};
  font-size: 22px;
  line-height: 2;
  white-space: pre-wrap;
  overflow-wrap: break-word;
}
.title { font-size: 18px; font-weight: 600; margin: 0 0 4px; }
.note { font-size: 12px; color: #666; margin: 0 0 16px; }
/* Every text starts a fresh page: the first sits at the top of the first sheet,
   each later one breaks before it. break-before is the modern spelling;
   page-break-before keeps older engines behaving the same. */
.page { break-after: auto; }
.page + .page { break-before: page; page-break-before: always; }
</style>
</head>
<body>
${pages}
</body>
</html>`;
}

/**
 * Open the print dialog for the given texts, each on its own page, typeset in
 * `font`. The user picks "Save as PDF" (or a printer) from the dialog — the only
 * way to keep a handwriting webfont faithful. Resolves once the dialog is shown;
 * the iframe lingers briefly so the browser can snapshot it before printing, then
 * is removed.
 */
export async function exportTextsToPdf(
  datasetName: string,
  texts: ReadText[],
  font: RaPrintFont,
  onProgress?: (p: RaExportProgress) => void
): Promise<void> {
  const t0 = performance.now();
  // Each line carries its own elapsed time — the split between our preparation
  // (ms) and the native print-to-PDF pipeline (the slow part on a big book,
  // per-page rasterisation of a variable font) is what tells you where to look.
  const log = (message: string) =>
    console.info(`${LOG_TAG} +${Math.round(performance.now() - t0)}ms ${message}`);
  const report = (step: string, message: string, percent: number) => {
    log(`${step}: ${message} (${percent}%)`);
    onProgress?.({ step, message, percent });
  };

  log(`export start: dataset="${datasetName}", texts=${texts.length}, font=${font.familyCss || UI_STACK}`);
  report("prepare", "Preparing print document…", 5);
  const printable = texts.filter((t) => t.content.trim());
  if (printable.length === 0) throw new Error("This dataset has no texts with content to export.");
  const totalChars = printable.reduce((n, t) => n + t.content.length, 0);
  log(`printable texts: ${printable.length}, total content chars: ${totalChars}`);

  const iframe = document.createElement("iframe");
  iframe.setAttribute("aria-hidden", "true");
  iframe.style.position = "fixed";
  iframe.style.left = "-10000px";
  iframe.style.top = "0";
  // A zero-sized or visibility-hidden frame is skipped by the print pipeline's
  // layout pass — large books then come out of "Save as PDF" as blank pages.
  // Keep the frame real (an A4-ish box) but parked off-screen instead.
  iframe.style.width = "794px";
  iframe.style.height = "1123px";
  iframe.style.border = "0";
  document.body.appendChild(iframe);

  try {
    const doc = iframe.contentDocument;
    if (!doc) throw new Error("Could not prepare the print document.");
    const html = buildDocument(datasetName, printable, font);
    log(`print document built: ${html.length} chars of HTML`);
    doc.open();
    doc.write(html);
    doc.close();

    // Wait for the script face to be ready, or the first paint prints in the
    // fallback and the handwriting is lost. `document.fonts` is what tells us.
    report("fonts", "Loading handwriting font…", 35);
    const fonts = (doc as unknown as { fonts?: { ready: Promise<unknown> } }).fonts;
    if (fonts?.ready) {
      await fonts.ready.catch(() => undefined);
    }
    log("document fonts ready");
    // Let the off-screen frame get at least one real paint before the print
    // snapshot is taken; without it a big document can render empty.
    await new Promise((resolve) => setTimeout(resolve, 150));

    report("layout", "Laying out pages…", 65);
    // One frame for layout to settle after the fonts swap in, then print. The
    // call is synchronous into the native dialog; cleanup is deferred so the
    // snapshot the dialog takes still has the document present.
    await new Promise((resolve) => requestAnimationFrame(resolve));
    iframe.contentWindow?.focus();
    report("print", "Opening print dialog…", 90);
    iframe.contentWindow?.print();
    log(`print() returned — dialog shown for ${printable.length} page(s)`);
    report("done", "Print dialog open — finish in the dialog.", 100);
  } catch (e) {
    log(`export failed: ${String(e)}`);
    throw e;
  } finally {
    // The dialog snapshots the frame when it opens; a short grace period is
    // enough to cancel a print started in the same tick. (Was 60s — a large
    // book's frame is heavy to hold in memory for no proven benefit.)
    setTimeout(() => iframe.remove(), 5_000);
  }
}
