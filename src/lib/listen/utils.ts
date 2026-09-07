import { Cue } from "../types";

/**
 * Replace word with underline, with the length of word
 */
export const hideWord = (content: string) => {
  const tip = content.replaceAll(/[a-zA-Z0-9äÄöÖüÜßé]/g, "_");
  let count = 0;
  let tip_with_count = "";
  for (const char of tip) {
    if (char === "_") {
      count++;
    } else {
      if (count > 0) {
        tip_with_count += `${count}`;
        count = 0;
      }
    }
    tip_with_count += char;
  }
  if (count > 0) {
    tip_with_count += `${count}`;
  }
  return tip_with_count;
};

/**
 * Used for simply check success, ignore non-words
 */
export const pureContent = (content: string) => {
  return content.replaceAll(/[^a-zA-Z0-9äÄöÖüÜßé]/g, "");
};

/**
 * Used for process content word by word
 */
export const splitContent = (content: string, onlyWord: boolean) => {
  const parts: { isWord: boolean; content: string }[] = [];
  let tmp = "";
  for (const char of content) {
    if (char.match(/[a-zA-Z0-9äÄöÖüÜßé]/)) {
      tmp += char;
      continue;
    }
    if (tmp !== "") {
      parts.push({ isWord: true, content: tmp });
      tmp = "";
    }
    if (!onlyWord) {
      parts.push({ isWord: false, content: char });
    }
  }
  if (tmp !== "") {
    parts.push({ isWord: true, content: tmp });
  }
  return parts;
};

export const isAudio = (src: string): boolean => {
  const AUDIO_EXT = ["mp3", "wav", "aac", "m4a", "ogg", "flac", "wma", "m4b"];
  try {
    const url = new URL(src);
    const pathname = url.pathname;
    const ext = pathname.split(".").pop()?.toLowerCase();
    return !!ext && AUDIO_EXT.includes(ext);
  } catch {
    const ext = src.split(".").pop()?.toLowerCase();
    return !!ext && AUDIO_EXT.includes(ext);
  }
};

export const playMediaPart = async (
  cue: Cue,
  media: HTMLMediaElement,
  extend: boolean = false
) => {
  if (!media) return;

  const start_ms = extend
    ? cue.start_ms >= 2000
      ? cue.start_ms - 2000
      : 0
    : cue.start_ms;
  const end_ms = extend ? cue.end_ms + 2000 : cue.end_ms;

  media.pause();
  media.currentTime = start_ms / 1000;
  await media.play().catch(() => {});

  const stop = () => {
    if (media.currentTime >= end_ms / 1000) {
      media.pause();
      return true;
    }
    return false;
  };

  const rafLoop = () => {
    if (!stop()) requestAnimationFrame(rafLoop);
  };
  requestAnimationFrame(rafLoop);
};
