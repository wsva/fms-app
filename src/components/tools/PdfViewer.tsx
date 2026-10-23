"use client";

/**
 * PDF viewer — the client-only rendering core of the PDF Reader tool.
 *
 * This module is the sole importer of `react-pdf`/`pdfjs` and is loaded through
 * a `dynamic(() => import("./PdfViewer"), { ssr: false })` boundary in
 * PdfReaderPage.tsx. That boundary is mandatory: importing `react-pdf` in Node
 * throws `ReferenceError: DOMMatrix is not defined`, and the static-export
 * "Collecting page data" step `require`s every statically-imported page module.
 * Keeping react-pdf behind an ssr:false boundary (exactly like WorkflowGraph is
 * kept out of WorkflowPage) means the prerender/collect path never evaluates it.
 *
 * Same engine as the website's `/tools/pdf_reader` (react-pdf over pdf.js, all
 * pages rendered into one scrollable column), adapted to the shell: the page
 * lives inside the single-page `flex-1 min-h-0` content area, so it owns its own
 * scroll container instead of the website's `min-h-screen` document flow, and the
 * chrome uses the theme tokens (bg-bg-card / border-border-default / text-text-*)
 * rather than the website's stone/amber palette.
 *
 * File picking rides a hidden `<input type="file">` exactly like SplitTextPage
 * — a Blob URL needs no dialog/fs capability and works in the browser preview
 * as well as in the webview.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { Document, Page, pdfjs } from "react-pdf";
import { FileText, Upload } from "lucide-react";
import { logError } from "@/lib/logger";
import { PageHeader, Toolbar } from "@/components/ui/toolbar";
import "react-pdf/dist/Page/TextLayer.css";
import "react-pdf/dist/Page/AnnotationLayer.css";

// Same webpack 5 asset-module worker wiring as the website: the worker file is
// emitted next to the chunk and resolved relative to this module's URL, so it
// stays local (no CDN) under the static export. This only ever runs in the
// client bundle (the ssr:false boundary keeps it out of Node).
pdfjs.GlobalWorkerOptions.workerSrc = new URL(
  "pdfjs-dist/build/pdf.worker.min.mjs",
  import.meta.url,
).toString();

const btnGhost =
  "px-3 py-1.5 rounded-md text-sm text-text-secondary hover:bg-bg-hover border border-border-default cursor-pointer";
const btnPrimary =
  "px-4 py-1.5 rounded-md text-sm font-medium bg-accent-bg text-white hover:opacity-90 cursor-pointer";
const inputCls =
  "w-20 px-2 py-1.5 rounded-md bg-bg-input border border-border-default text-sm text-text-primary focus:outline-none focus:border-accent";

export default function PdfViewer() {
  const [pdfUrl, setPdfUrl] = useState<string | null>(null);
  const [fileName, setFileName] = useState<string>("");
  const [numPages, setNumPages] = useState<number>(0);
  const [fullWidth, setFullWidth] = useState<boolean>(false);
  const [pageWidth, setPageWidth] = useState<number>(800);
  const [jumpInput, setJumpInput] = useState<string>("");
  const [error, setError] = useState<string>("");
  const containerRef = useRef<HTMLDivElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const pageRefs = useRef<(HTMLDivElement | null)[]>([]);

  // Measure against the scroll container, not the window: sidebar collapse,
  // workspace-header toggles and global page zoom all change the content width
  // without firing a window resize.
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const measure = () => {
      const w = el.clientWidth;
      setPageWidth(fullWidth ? w : Math.min(w, 800));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [fullWidth, pdfUrl]);

  // Release the blob URL when the shell finally unmounts the page.
  useEffect(() => {
    return () => {
      if (pdfUrl) URL.revokeObjectURL(pdfUrl);
    };
  }, [pdfUrl]);

  const onFileChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const file = e.target.files?.[0];
      // Reset the input so re-picking the same file still fires change.
      e.target.value = "";
      if (!file) return;
      if (file.type !== "application/pdf" && !file.name.toLowerCase().endsWith(".pdf")) {
        setError(`"${file.name}" is not a PDF file`);
        return;
      }
      if (pdfUrl) URL.revokeObjectURL(pdfUrl);
      setPdfUrl(URL.createObjectURL(file));
      setFileName(file.name);
      setNumPages(0);
      setError("");
      pageRefs.current = [];
    },
    [pdfUrl]
  );

  const jumpToPage = (n: number) => {
    const idx = Math.max(1, Math.min(n, numPages)) - 1;
    pageRefs.current[idx]?.scrollIntoView({ behavior: "smooth", block: "start" });
  };

  const handleJump = () => {
    const n = parseInt(jumpInput);
    if (!isNaN(n)) jumpToPage(n);
  };

  return (
    <div className="flex flex-col h-full w-full min-w-0 bg-bg-body text-text-primary overflow-hidden">
      <PageHeader title="PDF Reader" />
      {/* Toolbar */}
      <Toolbar className="flex-wrap">
        <button onClick={() => fileInputRef.current?.click()} className={btnPrimary}>
          {pdfUrl ? "Change PDF" : "Open PDF"}
        </button>
        {fileName && (
          <span className="text-sm text-text-tertiary truncate max-w-xs" title={fileName}>
            {fileName}
          </span>
        )}
        <input
          ref={fileInputRef}
          type="file"
          accept="application/pdf,.pdf"
          className="hidden"
          onChange={onFileChange}
        />

        {pdfUrl && numPages > 0 && (
          <>
            <div className="h-4 w-px bg-border-default" />

            <button
              onClick={() => setFullWidth((v) => !v)}
              className={`${btnGhost} ${fullWidth ? "!bg-accent-bg !text-white !border-accent-bg" : ""}`}
            >
              Full width
            </button>

            <div className="h-4 w-px bg-border-default" />

            <button onClick={() => jumpToPage(1)} className={btnGhost}>
              ↑ Top
            </button>
            <button onClick={() => jumpToPage(numPages)} className={btnGhost}>
              ↓ Bottom
            </button>

            <div className="h-4 w-px bg-border-default" />

            <div className="flex items-center gap-1.5">
              <input
                type="number"
                min={1}
                max={numPages}
                value={jumpInput}
                onChange={(e) => setJumpInput(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && handleJump()}
                placeholder={`1–${numPages}`}
                className={inputCls}
              />
              <button onClick={handleJump} className={btnGhost}>
                Go
              </button>
            </div>

            <span className="text-xs text-text-tertiary ml-auto">{numPages} pages</span>
          </>
        )}
      </Toolbar>

      {error && (
        <div className="px-3 py-2 text-sm text-error-text border-b border-border-default bg-bg-card">{error}</div>
      )}

      {/* Document */}
      {pdfUrl ? (
        <div ref={containerRef} className="flex-1 min-h-0 overflow-y-auto flex flex-col items-center gap-2 p-2">
          <Document
            file={pdfUrl}
            onLoadSuccess={({ numPages: n }) => setNumPages(n)}
            onLoadError={(err) => {
              logError(`PDF load failed: ${err?.message ?? String(err)}`);
              setError("Could not load this PDF — it may be corrupted or password-protected.");
            }}
            loading={<div className="text-text-tertiary text-sm py-8">Loading PDF…</div>}
          >
            {Array.from({ length: numPages }, (_, i) => (
              <div
                key={i}
                ref={(el) => {
                  pageRefs.current[i] = el;
                }}
                className="mb-4 shadow-md rounded overflow-hidden bg-bg-card"
              >
                <Page
                  pageNumber={i + 1}
                  width={pageWidth}
                  renderTextLayer={true}
                  renderAnnotationLayer={true}
                />
              </div>
            ))}
          </Document>
        </div>
      ) : (
        <div className="flex-1 min-h-0 overflow-y-auto flex flex-col items-center justify-center py-24 text-text-tertiary select-none">
          <FileText width={48} height={48} strokeWidth={1.5} />
          <div className="mt-3 flex items-center gap-2 text-sm">
            <Upload width={16} height={16} />
            Open a PDF to start reading
          </div>
        </div>
      )}
    </div>
  );
}
