"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Check, Crop, RotateCcw, RotateCw, Undo2, X } from "lucide-react";
import { tbGhost } from "@/lib/ui/toolbar";
import { ToolbarIconButton } from "@/components/ui/toolbar";

interface ImageEditorProps {
  /** Source image: a data URL or an asset URL from convertFileSrc(). */
  src: string | null;
  /** Called with the current PNG data URL whenever the working image changes. */
  onImageReady: (dataUrl: string | null) => void;
  /** Surfaces load failures to the page error banner. */
  onError?: (message: string) => void;
}

/** A rectangle in fractions (0..1) of the image it is applied to. */
interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** One rendered state of the image: the PNG plus the size it was drawn at. */
interface Frame {
  url: string;
  width: number;
  height: number;
}

/** The whole derivation: the rotated page, and the crop of it if there is one. */
interface View {
  rotated: Frame;
  cropped: Frame | null;
}

/** A selection smaller than this is a stray click, not a crop. */
const MIN_CROP = 0.02;
/** Rotation slider steps, in degrees. */
const ANGLE_STEP = 0.5;

// ---------------------------------------------------------------------------
// Canvas helpers
// ---------------------------------------------------------------------------

function loadImage(url: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    // Required for asset-protocol URLs so the canvas stays exportable.
    if (!url.startsWith("data:")) img.crossOrigin = "anonymous";
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error("Failed to load image"));
    img.src = url;
  });
}

/** The image at its own size, as a canvas to transform from. */
function sourceCanvas(img: HTMLImageElement): HTMLCanvasElement {
  const canvas = document.createElement("canvas");
  canvas.width = img.naturalWidth;
  canvas.height = img.naturalHeight;
  canvas.getContext("2d")?.drawImage(img, 0, 0);
  return canvas;
}

/**
 * Rotate about the centre by any angle, onto white.
 *
 * White rather than transparent: OCR engines read an unpainted (transparent)
 * pixel as black, so a rotated page on a transparent ground turns its own
 * margins into a black frame and the recogniser hunts for text in it.
 */
function rotateCanvas(source: HTMLCanvasElement, degrees: number): HTMLCanvasElement {
  const radians = (degrees * Math.PI) / 180;
  const cos = Math.abs(Math.cos(radians));
  const sin = Math.abs(Math.sin(radians));
  const width = Math.round(source.width * cos + source.height * sin);
  const height = Math.round(source.width * sin + source.height * cos);

  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (ctx) {
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, width, height);
    ctx.translate(width / 2, height / 2);
    ctx.rotate(radians);
    ctx.drawImage(source, -source.width / 2, -source.height / 2);
  }
  return canvas;
}

/** Cut a fractional rectangle out of `source`. */
function cropCanvas(source: HTMLCanvasElement, rect: Rect): HTMLCanvasElement {
  const x = Math.round(rect.x * source.width);
  const y = Math.round(rect.y * source.height);
  const width = Math.max(1, Math.round(rect.w * source.width));
  const height = Math.max(1, Math.round(rect.h * source.height));

  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  canvas.getContext("2d")?.drawImage(source, x, y, width, height, 0, 0, width, height);
  return canvas;
}

const clamp01 = (value: number) => Math.min(1, Math.max(0, value));

/** Keep an angle in (-180, 180]: the slider's own range, whatever it is handed. */
function normalizeAngle(degrees: number) {
  const wrapped = (((degrees + 180) % 360) + 360) % 360;
  return Math.round((wrapped - 180) * 10) / 10;
}

function rectBetween(x1: number, y1: number, x2: number, y2: number): Rect {
  return {
    x: Math.min(x1, x2),
    y: Math.min(y1, y2),
    w: Math.abs(x2 - x1),
    h: Math.abs(y2 - y1),
  };
}

/** The box the image occupies inside its pane: `object-contain`, centred. */
function containBox(area: { w: number; h: number }, frame: Frame) {
  const scale = Math.min(area.w / frame.width, area.h / frame.height);
  const width = frame.width * scale;
  const height = frame.height * scale;
  return { left: (area.w - width) / 2, top: (area.h - height) / 2, width, height };
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/**
 * The image pane: normalises whatever was picked into a PNG, and lets the page
 * be straightened and cropped before it is recognised.
 *
 * The transforms are kept as *state* (an angle, a rectangle) and the pixels are
 * derived from them, so every edit is reversible and the crop always refers to
 * the rotated page the user is looking at. The angle resets the crop: a
 * rectangle drawn on one orientation is meaningless on another.
 */
export default function ImageEditor({ src, onImageReady, onError }: ImageEditorProps) {
  // The normalised source, always a same-origin PNG data URL.
  const [base, setBase] = useState<string | null>(null);
  // Straightening angle in degrees, and the crop in fractions of the rotated
  // page. Both null/0 mean "as acquired".
  const [rotation, setRotation] = useState(0);
  const [crop, setCrop] = useState<Rect | null>(null);

  // Crop editing: while on, the pane shows the *whole* rotated page with the
  // rectangle over it, so a new drag simply replaces the old rectangle.
  const [cropMode, setCropMode] = useState(false);
  const [selection, setSelection] = useState<Rect | null>(null);
  const dragFrom = useRef<{ x: number; y: number } | null>(null);

  const [view, setView] = useState<View | null>(null);
  const areaRef = useRef<HTMLDivElement>(null);
  const [area, setArea] = useState({ w: 0, h: 0 });

  // Keep latest callbacks without re-triggering the derivation effect.
  const onReadyRef = useRef(onImageReady);
  onReadyRef.current = onImageReady;
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;

  // Acquire: normalise the source and drop every edit of the previous image.
  useEffect(() => {
    setRotation(0);
    setCrop(null);
    setCropMode(false);
    setSelection(null);
    if (!src) {
      setBase(null);
      return;
    }
    let active = true;
    loadImage(src)
      .then((img) => {
        if (!active) return;
        setBase(sourceCanvas(img).toDataURL("image/png"));
      })
      .catch((e) => {
        if (!active) return;
        setBase(null);
        onErrorRef.current?.(
          `Could not read the image: ${e instanceof Error ? e.message : String(e)}`
        );
      });
    return () => {
      active = false;
    };
  }, [src]);

  // Derive the pixels from (base, angle, crop).
  useEffect(() => {
    if (!base) {
      setView(null);
      onReadyRef.current(null);
      return;
    }
    let active = true;
    (async () => {
      try {
        const img = await loadImage(base);
        const full = sourceCanvas(img);
        const rotated = rotation === 0 ? full : rotateCanvas(full, rotation);
        const rotatedUrl = rotated === full ? base : rotated.toDataURL("image/png");

        const cropped = crop ? cropCanvas(rotated, crop) : null;
        if (!active) return;

        setView({
          rotated: { url: rotatedUrl, width: rotated.width, height: rotated.height },
          cropped: cropped
            ? { url: cropped.toDataURL("image/png"), width: cropped.width, height: cropped.height }
            : null,
        });
        onReadyRef.current(cropped ? cropped.toDataURL("image/png") : rotatedUrl);
      } catch (e) {
        if (!active) return;
        setView(null);
        onReadyRef.current(null);
        onErrorRef.current?.(
          `Could not prepare the image: ${e instanceof Error ? e.message : String(e)}`
        );
      }
    })();
    return () => {
      active = false;
    };
  }, [base, rotation, crop]);

  // The pane's own size: the image is drawn inside it with `object-contain`
  // semantics, and the crop overlay has to sit exactly on the drawn pixels.
  useEffect(() => {
    const element = areaRef.current;
    if (!element) return;
    const measure = () => setArea({ w: element.clientWidth, h: element.clientHeight });
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    measure();
    return () => observer.disconnect();
  }, [view]);

  // What the pane draws: the whole page while cropping, the crop otherwise.
  const shown = cropMode ? view?.rotated : (view?.cropped ?? view?.rotated);
  const box = useMemo(
    () => (shown && area.w > 0 && area.h > 0 ? containBox(area, shown) : null),
    [shown, area]
  );

  const setAngle = useCallback((degrees: number) => {
    setRotation(normalizeAngle(degrees));
    // A rectangle drawn on one orientation does not describe the next one.
    setCrop(null);
    setSelection(null);
  }, []);

  const startCrop = useCallback(() => {
    // Enter with the applied rectangle selected, so it can be judged again.
    setSelection(crop);
    setCropMode(true);
  }, [crop]);

  const cancelCrop = useCallback(() => {
    setCropMode(false);
    setSelection(null);
  }, []);

  const applyCrop = useCallback(() => {
    if (!selection || selection.w < MIN_CROP || selection.h < MIN_CROP) return;
    setCrop(selection);
    setCropMode(false);
    setSelection(null);
  }, [selection]);

  const resetEdits = useCallback(() => {
    setRotation(0);
    setCrop(null);
    setCropMode(false);
    setSelection(null);
  }, []);

  const pointOf = (event: React.PointerEvent<HTMLDivElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    return {
      x: clamp01((event.clientX - rect.left) / rect.width),
      y: clamp01((event.clientY - rect.top) / rect.height),
    };
  };

  const edited = rotation !== 0 || crop !== null;

  return (
    <div className="h-full min-h-0 flex flex-col gap-2">
      <div
        ref={areaRef}
        className="relative flex-1 min-h-0 overflow-hidden rounded-lg border border-border-default bg-bg-muted"
      >
        {shown && box ? (
          <>
            <img
              src={shown.url}
              alt="OCR source"
              draggable={false}
              className="absolute select-none"
              style={{ left: box.left, top: box.top, width: box.width, height: box.height }}
            />
            {cropMode && (
              <div
                className="absolute touch-none cursor-crosshair"
                style={{ left: box.left, top: box.top, width: box.width, height: box.height }}
                onPointerDown={(event) => {
                  event.currentTarget.setPointerCapture(event.pointerId);
                  const point = pointOf(event);
                  dragFrom.current = point;
                  setSelection({ x: point.x, y: point.y, w: 0, h: 0 });
                }}
                onPointerMove={(event) => {
                  const from = dragFrom.current;
                  if (!from) return;
                  const point = pointOf(event);
                  setSelection(rectBetween(from.x, from.y, point.x, point.y));
                }}
                onPointerUp={() => {
                  dragFrom.current = null;
                }}
                onPointerCancel={() => {
                  dragFrom.current = null;
                }}
              >
                {selection && selection.w > 0 && selection.h > 0 && (
                  <div
                    className="absolute border-2 border-accent"
                    style={{
                      left: `${selection.x * 100}%`,
                      top: `${selection.y * 100}%`,
                      width: `${selection.w * 100}%`,
                      height: `${selection.h * 100}%`,
                      // One shadow dims everything the crop would discard.
                      boxShadow: "0 0 0 9999px rgba(0,0,0,0.45)",
                    }}
                  />
                )}
              </div>
            )}
          </>
        ) : (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 p-6 text-center">
            <Crop size={40} className="text-text-tertiary opacity-40" />
            <p className="text-sm text-text-tertiary">
              No image yet. Use the toolbar to select or screenshot one, or drop a file here.
            </p>
          </div>
        )}
      </div>

      {/* The transforms. Crop mode replaces them with its own two verbs, so the
          bar never offers an action that would move what is being selected. */}
      {shown && (
        <div className="shrink-0 flex flex-wrap items-center gap-1.5">
          {cropMode ? (
            <>
              <span className="flex-1 min-w-0 text-xs text-text-tertiary truncate">
                Drag on the image to select what to keep.
              </span>
              <button
                type="button"
                onClick={applyCrop}
                disabled={!selection || selection.w < MIN_CROP || selection.h < MIN_CROP}
                className={`${tbGhost} text-xs`}
                title="Keep the selected area"
              >
                <Check size={14} />
                Apply
              </button>
              <button
                type="button"
                onClick={cancelCrop}
                className={`${tbGhost} text-xs`}
                title="Leave the image as it is"
              >
                <X size={14} />
                Cancel
              </button>
            </>
          ) : (
            <>
              <button
                type="button"
                onClick={startCrop}
                className={`${tbGhost} text-xs`}
                title={crop ? "Pick a different area" : "Cut the page down to what matters"}
              >
                <Crop size={14} />
                {crop ? "Re-crop" : "Crop"}
              </button>
              <ToolbarIconButton
                icon={RotateCcw}
                ariaLabel="Rotate 90° counter-clockwise"
                onClick={() => setAngle(rotation - 90)}
              />
              <ToolbarIconButton
                icon={RotateCw}
                ariaLabel="Rotate 90° clockwise"
                onClick={() => setAngle(rotation + 90)}
              />
              <input
                type="range"
                min={-180}
                max={180}
                step={ANGLE_STEP}
                value={rotation}
                onChange={(event) => setAngle(Number(event.target.value))}
                className="flex-1 min-w-[72px] max-w-[200px] accent-accent cursor-pointer"
                aria-label="Straighten angle in degrees"
                title="Straighten: any angle from -180° to 180°"
              />
              <span className="w-12 shrink-0 text-right text-xs tabular-nums text-text-tertiary">
                {rotation.toFixed(1)}°
              </span>
              <ToolbarIconButton
                icon={Undo2}
                ariaLabel="Reset rotation and crop"
                onClick={resetEdits}
                disabled={!edited}
                className="ml-auto"
              />
            </>
          )}
        </div>
      )}
    </div>
  );
}
