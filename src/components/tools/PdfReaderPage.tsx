"use client";

/**
 * PDF Reader tool — the app-side port of the website's `/tools/pdf_reader`.
 *
 * This shell is statically imported by the single-page app/page.tsx, so it must
 * stay free of any `react-pdf`/`pdfjs` reference: importing react-pdf in Node
 * throws `DOMMatrix is not defined`, which would break the static-export
 * "Collecting page data" step. All react-pdf code lives in ./PdfViewer, loaded
 * through an `ssr: false` dynamic boundary — the same convention WorkflowPage
 * uses to keep @xyflow/react out of the prerender path.
 */
import dynamic from "next/dynamic";

const PdfViewer = dynamic(() => import("./PdfViewer"), {
  ssr: false,
  loading: () => (
    <div className="flex flex-col h-full w-full items-center justify-center bg-bg-body text-text-tertiary text-sm">
      Loading PDF reader…
    </div>
  ),
});

export default function PdfReaderPage() {
  return <PdfViewer />;
}
