import { Cue } from "../types";

const pad = (num: number, size = 2): string => num.toString().padStart(size, "0");

export const validateVttTime = (timeStr: string): RegExpMatchArray | null => {
  return timeStr.match(/^(\d+):(\d{2}):(\d{2})\.(\d{3})$/);
};

export const parseVttTime = (timeStr: string): number => {
  const match = validateVttTime(timeStr);
  if (!match) return 0;
  const [, h, m, s, ms] = match.map(Number);
  return (h * 3600 + m * 60 + s) * 1000 + ms;
};

export const formatVttTime = (time_ms: number): string => {
  const h = Math.floor(time_ms / 3600000);
  const m = Math.floor((time_ms % 3600000) / 60000);
  const s = Math.floor((time_ms % 60000) / 1000);
  const ms = time_ms % 1000;
  return `${pad(h)}:${pad(m)}:${pad(s)}.${pad(ms, 3)}`;
};

export function parseVTT(vttText: string, contain_translation: boolean): Cue[] {
  vttText = vttText.replace(/^\uFEFF/, "").trim();
  vttText = vttText.replace(/^WEBVTT[\s\S]*?\n\n/, "");

  const cueBlocks = vttText
    .split(/\n\s*\n/)
    .map((block) => block.trim())
    .filter(Boolean);

  const cues: Cue[] = [];

  for (const block of cueBlocks) {
    const lines = block.split("\n").map((l) => l.trim());
    if (lines.length < 2) continue;

    const timeMatch = lines[0].match(
      /(\d{2}:\d{2}:\d{2}\.\d{3})\s*-->\s*(\d{2}:\d{2}:\d{2}\.\d{3})/
    );
    if (!timeMatch) continue;

    const start = parseVttTime(timeMatch[1]);
    const end = parseVttTime(timeMatch[2]);

    let textLines: string[] = [];
    if (contain_translation) {
      if (lines.length > 1) textLines.push(lines[1]);
    } else {
      textLines = lines.slice(1);
    }

    cues.push({
      uuid: crypto.randomUUID().replaceAll("-", ""),
      subtitle_uuid: "",
      order_num: cues.length + 1,
      start_ms: start,
      end_ms: end,
      content: textLines.join(" "),
      reference: null,
      active: false,
    });
  }

  return cues;
}

export function parseSRT(srtText: string, contain_translation: boolean = false): Cue[] {
  srtText = srtText.replace(/^\uFEFF/, "").trim();

  const blocks = srtText
    .split(/\r?\n\r?\n/)
    .map((block) => block.trim())
    .filter(Boolean);

  const parseTime = (timeStr: string): number => {
    const match = timeStr.match(/(\d+):(\d{2}):(\d{2}),(\d{3})/);
    if (!match) return 0;
    const [, h, m, s, ms] = match.map(Number);
    return (h * 3600 + m * 60 + s) * 1000 + ms;
  };

  const cues: Cue[] = [];

  for (const block of blocks) {
    const lines = block.split(/\r?\n/).map((l) => l.trim());
    if (lines.length < 2) continue;

    let timeLine = "";
    if (lines[0].includes("-->")) {
      timeLine = lines[0];
    } else if (lines[1]?.includes("-->")) {
      timeLine = lines[1];
      lines.shift();
    } else continue;

    const timeMatch = timeLine.match(
      /(\d{2}:\d{2}:\d{2},\d{3})\s*-->\s*(\d{2}:\d{2}:\d{2},\d{3})/
    );
    if (!timeMatch) continue;

    const start = parseTime(timeMatch[1]);
    const end = parseTime(timeMatch[2]);

    let textLines: string[] = [];
    if (contain_translation) {
      if (lines.length > 1) textLines.push(lines[1]);
    } else {
      textLines = lines.slice(1);
    }

    cues.push({
      uuid: crypto.randomUUID().replaceAll("-", ""),
      subtitle_uuid: "",
      order_num: cues.length + 1,
      start_ms: start,
      end_ms: end,
      content: textLines.join(" "),
      reference: null,
      active: false,
    });
  }

  return cues;
}

export function buildVTT(cues: Cue[]): string {
  const header = "WEBVTT\n\n";
  const body = cues
    .map((cue) => {
      const start = formatVttTime(cue.start_ms);
      const end = formatVttTime(cue.end_ms);
      return [`${start} --> ${end}`, cue.content, ""].join("\n");
    })
    .join("\n");
  return header + body.trim() + "\n";
}

export const mergeCues = (item_list: Cue[]): Cue[] => {
  if (!item_list || item_list.length === 0) return [];

  const shouldMerge = (prev: Cue, current: Cue) => {
    return (
      prev.content.match(/[a-zA-Z0-9äÄöÖüÜßé,:-]$/) &&
      current.content.match(/^[0-9a-zA-ZäÄöÖüÜßé]/)
    );
  };

  let mergedList = [...item_list];
  let changed = true;

  while (changed) {
    const newList: Cue[] = [];
    changed = false;

    for (const item of mergedList) {
      const lastItem = newList[newList.length - 1];
      if (lastItem && shouldMerge(lastItem, item)) {
        lastItem.end_ms = item.end_ms;
        lastItem.content += " " + item.content;
        changed = true;
      } else {
        newList.push({ ...item });
      }
    }

    mergedList = newList;
  }

  mergedList.forEach((cue, i) => {
    cue.order_num = i + 1;
  });

  return mergedList;
};
