"use client";

import { useEffect, useRef, useState } from "react";
import { Crop } from "lucide-react";

interface ImageEditorProps {
  /** Source image: a data URL or an asset URL from convertFileSrc(). */
  src: string | null;
  /** Called with the current PNG data URL whenever the working image changes. */
  onImageReady: (dataUrl: string | null) => void;
  /** Surfaces load failures to the page error banner. */
  onError?: (message: string) => void;
}

// ---------------------------------------------------------------------------
// Canvas helper
// ---------------------------------------------------------------------------

/** Load `src` and re-encode it as a same-origin PNG data URL. */
function normalizeToDataUrl(url: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    // Required for asset-protocol URLs so the canvas stays exportable.
    if (!url.startsWith("data:")) img.crossOrigin = "anonymous";
    img.onload = () => {
      const canvas = document.createElement("canvas");
      canvas.width = img.naturalWidth;
      canvas.height = img.naturalHeight;
      const ctx = canvas.getContext("2d");
      if (ctx) ctx.drawImage(img, 0, 0);
      try {
        resolve(canvas.toDataURL("image/png"));
      } catch (e) {
        reject(e);
      }
    };
    img.onerror = () => reject(new Error("Failed to load image"));
    img.src = url;
  });
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function ImageEditor({ src, onImageReady, onError }: ImageEditorProps) {
  // Current working image, always a same-origin PNG data URL.
  const [working, setWorking] = useState<string | null>(null);

  // Keep latest callbacks without re-triggering the normalization effect.
  const onReadyRef = useRef(onImageReady);
  onReadyRef.current = onImageReady;
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;

  // Normalize the source whenever a new image is acquired.
  useEffect(() => {
    if (!src) {
      setWorking(null);
      onReadyRef.current(null);
      return;
    }
    let active = true;
    normalizeToDataUrl(src)
      .then((dataUrl) => {
        if (!active) return;
        setWorking(dataUrl);
        onReadyRef.current(dataUrl);
      })
      .catch((e) => {
        if (!active) return;
        setWorking(null);
        onReadyRef.current(null);
        onErrorRef.current?.(
          `Could not read the image: ${e instanceof Error ? e.message : String(e)}`
        );
      });
    return () => {
      active = false;
    };
  }, [src]);

  return (
    <div className="rounded-lg border border-border-default bg-bg-muted p-3 flex items-center justify-center min-h-[240px] overflow-auto">
      {working ? (
        <img
          src={working}
          alt="OCR source"
          className="block max-w-full max-h-[52vh] object-contain rounded"
        />
      ) : (
        <div className="text-center text-text-tertiary py-10 px-4">
          <Crop size={40} className="mx-auto mb-3 opacity-40" />
          <p className="text-sm">
            No image yet. Use the buttons above to select or screenshot an image.
          </p>
        </div>
      )}
    </div>
  );
}
