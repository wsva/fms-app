"use client";

import { useCallback, useEffect, useRef } from "react";

export type WaveformData = {
    version?: number;
    channels?: number;
    sample_rate?: number;
    samples_per_pixel?: number;
    bits?: number;
    length: number;
    data: number[];
};

type Props = {
    peaks: WaveformData;
    videoRef: React.RefObject<HTMLVideoElement | null>;
    selection?: {
        start: number;
        end: number;
    };
    onSelectionChange?: (start: number, end: number) => void;
};

// Theme-aware colors via CSS custom properties — resolved at draw time
const getColors = () => {
    const style = getComputedStyle(document.documentElement);
    const accent = style.getPropertyValue("--accent").trim() || "#0EA89A";
    const borderLight = style.getPropertyValue("--border-light").trim() || "#cccccc";
    return {
        unplayed: borderLight,
        played: accent,
        selectionFill: "rgba(34,197,94,0.20)",
        selectionEdge: "#16a34a",
        hoverLine: "rgba(37,99,235,0.75)",
    };
};

const fmtTime = (sec: number): string => {
    if (!isFinite(sec) || sec < 0) return "00:00:00.000";
    const h = Math.floor(sec / 3600);
    const m = Math.floor((sec % 3600) / 60);
    const s = Math.floor(sec % 60);
    const ms = Math.min(999, Math.round((sec % 1) * 1000));
    return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}.${String(ms).padStart(3, "0")}`;
};

function getVisibleRange(duration: number, start: number, stop: number) {
    const hasSelection = stop > start;
    if (!hasSelection) {
        return { visibleStart: 0, visibleEnd: duration };
    }
    const length = stop - start;
    const padding = Math.max(1, length * 0.2);
    return {
        visibleStart: Math.max(0, start - padding),
        visibleEnd: Math.min(duration, stop + padding),
    };
}

export default function WaveformCanvas({ peaks, videoRef, selection }: Props) {
    const canvasRef = useRef<HTMLCanvasElement>(null);
    const peaksRef = useRef(peaks);
    peaksRef.current = peaks;

    const startRef = useRef(0);
    const stopRef = useRef(0);

    const hoverXRef = useRef<number | null>(null);
    const drawRafRef = useRef<number>(0);
    const seekRafRef = useRef<number>(0);

    const draw = useCallback(() => {
        const canvas = canvasRef.current;
        if (!canvas) return;

        const ctx = canvas.getContext("2d");
        if (!ctx) return;

        const width = canvas.clientWidth;
        const height = canvas.clientHeight;
        if (!width || !height) return;

        ctx.clearRect(0, 0, width, height);

        const { data, bits = 8, channels = 1, length: numSamples } = peaksRef.current;
        if (!data || numSamples === 0) return;

        const video = videoRef.current;
        const duration =
            video && Number.isFinite(video.duration) && video.duration > 0
                ? video.duration
                : 0;
        const scale = bits === 8 ? 128 : 32768;
        const stride = channels * 2;

        let visibleStart = 0;
        let visibleEnd = duration;

        if (duration > 0) {
            const range = getVisibleRange(duration, startRef.current, stopRef.current);
            visibleStart = range.visibleStart;
            visibleEnd = range.visibleEnd;
        }

        const visibleDuration = Math.max(0.001, visibleEnd - visibleStart);

        const playheadX =
            duration > 0
                ? Math.round(((video!.currentTime - visibleStart) / visibleDuration) * width)
                : 0;

        const sampleStart =
            duration > 0 ? Math.floor((visibleStart / duration) * numSamples) : 0;
        const sampleEnd =
            duration > 0
                ? Math.max(sampleStart + 1, Math.floor((visibleEnd / duration) * numSamples))
                : numSamples;

        const visibleSamples = Math.max(1, sampleEnd - sampleStart);
        const colors = getColors();

        for (let px = 0; px < width; px++) {
            const idx = Math.min(
                sampleStart + Math.floor((px / width) * visibleSamples),
                sampleEnd - 1,
            );

            const min = data[idx * stride] / scale;
            const max = data[idx * stride + 1] / scale;
            const yTop = Math.floor(height / 2 - max * (height / 2) * 0.9);
            const yBottom = Math.ceil(height / 2 - min * (height / 2) * 0.9);

            ctx.fillStyle = px <= playheadX ? colors.played : colors.unplayed;
            ctx.fillRect(px, yTop, 1, Math.max(1, yBottom - yTop));
        }

        // Selection overlay
        if (duration > 0 && stopRef.current > startRef.current) {
            const x0 = ((startRef.current - visibleStart) / visibleDuration) * width;
            const x1 = ((stopRef.current - visibleStart) / visibleDuration) * width;
            ctx.fillStyle = colors.selectionFill;
            ctx.fillRect(x0, 0, x1 - x0, height);
            ctx.fillStyle = colors.selectionEdge;
            ctx.fillRect(x0, 0, 2, height);
            ctx.fillRect(x1 - 1, 0, 2, height);
        }

        // Playhead line
        ctx.fillStyle = colors.played;
        ctx.fillRect(Math.max(0, playheadX - 1), 0, 2, height);

        // Hover time tooltip
        if (hoverXRef.current !== null && duration > 0) {
            ctx.save();
            ctx.strokeStyle = colors.hoverLine;
            ctx.lineWidth = 1;
            ctx.beginPath();
            ctx.moveTo(hoverXRef.current, 0);
            ctx.lineTo(hoverXRef.current, height);
            ctx.stroke();

            const hoverRatio = hoverXRef.current / width;
            const hoverTime = visibleStart + hoverRatio * visibleDuration;
            const text = fmtTime(hoverTime);
            ctx.font = "11px system-ui,sans-serif";
            ctx.textBaseline = "middle";
            const paddingX = 6;
            const textWidth = ctx.measureText(text).width;
            const boxWidth = textWidth + paddingX * 2;
            const boxHeight = 18;
            let boxX = hoverXRef.current + 8;
            if (boxX + boxWidth > width) boxX = hoverXRef.current - boxWidth - 8;
            const boxY = 2;
            ctx.fillStyle = "rgba(17,24,39,.92)";
            ctx.beginPath();
            ctx.roundRect(boxX, boxY, boxWidth, boxHeight, 4);
            ctx.fill();
            ctx.fillStyle = "#ffffff";
            ctx.fillText(text, boxX + paddingX, boxY + boxHeight / 2);
            ctx.restore();
        }
    }, [videoRef]);

    const scheduleDraw = useCallback(() => {
        cancelAnimationFrame(drawRafRef.current!);
        drawRafRef.current = requestAnimationFrame(draw);
    }, [draw]);

    // Resize observer
    useEffect(() => {
        const canvas = canvasRef.current;
        if (!canvas) return;

        const resize = () => {
            const dpr = window.devicePixelRatio || 1;
            canvas.width = canvas.clientWidth * dpr;
            canvas.height = canvas.clientHeight * dpr;
            const ctx = canvas.getContext("2d");
            if (!ctx) return;
            ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
            scheduleDraw();
        };
        resize();

        const ro = new ResizeObserver(resize);
        ro.observe(canvas);
        return () => ro.disconnect();
    }, [scheduleDraw]);

    // Redraw on peaks change
    useEffect(() => {
        scheduleDraw();
    }, [peaks, scheduleDraw]);

    // Selection update
    useEffect(() => {
        if (!selection) return;
        startRef.current = selection.start;
        stopRef.current = selection.end;
        scheduleDraw();
    }, [selection?.start, selection?.end, scheduleDraw]);

    // Playback sync
    useEffect(() => {
        const video = videoRef.current;
        if (!video) return;

        let raf = 0;
        const render = () => {
            draw();
            raf = requestAnimationFrame(render);
        };
        const onPlay = () => {
            cancelAnimationFrame(raf);
            render();
        };
        const onPause = () => cancelAnimationFrame(raf);

        video.addEventListener("play", onPlay);
        video.addEventListener("pause", onPause);
        video.addEventListener("seeked", scheduleDraw);
        video.addEventListener("loadedmetadata", scheduleDraw);

        return () => {
            cancelAnimationFrame(raf);
            video.removeEventListener("play", onPlay);
            video.removeEventListener("pause", onPause);
            video.removeEventListener("seeked", scheduleDraw);
            video.removeEventListener("loadedmetadata", scheduleDraw);
        };
    }, [draw, scheduleDraw, videoRef]);

    const seek = useCallback(
        (e: React.MouseEvent<HTMLCanvasElement>) => {
            const video = videoRef.current;
            if (!video || !Number.isFinite(video.duration) || video.duration <= 0) return;

            const rect = e.currentTarget.getBoundingClientRect();
            const ratio = (e.clientX - rect.left) / rect.width;
            const { visibleStart, visibleEnd } = getVisibleRange(
                video.duration,
                startRef.current,
                stopRef.current,
            );
            video.currentTime = visibleStart + ratio * (visibleEnd - visibleStart);
        },
        [videoRef],
    );

    const throttledSeek = useCallback(
        (e: React.MouseEvent<HTMLCanvasElement>) => {
            cancelAnimationFrame(seekRafRef.current!);
            seekRafRef.current = requestAnimationFrame(() => seek(e));
        },
        [seek],
    );

    return (
        <div className="flex flex-col gap-1 bg-bg-muted rounded-sm">
            <canvas
                ref={canvasRef}
                className="w-full cursor-pointer rounded"
                style={{ height: "44px", display: "block" }}
                onClick={seek}
                onMouseMove={(e) => {
                    const rect = e.currentTarget.getBoundingClientRect();
                    hoverXRef.current = e.clientX - rect.left;
                    scheduleDraw();
                    if (e.buttons === 1) throttledSeek(e);
                }}
                onMouseLeave={() => {
                    hoverXRef.current = null;
                    scheduleDraw();
                }}
            />
        </div>
    );
}
