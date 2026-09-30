import { createElement, type ReactNode } from "react";
import { TOC } from "./toc";

// Only allow safe URL schemes; block javascript: and data: hrefs.
// Also allow fms-app://wiki/ for internal wiki links.
function sanitizeUrl(url: string): string {
    const s = url.trim();
    return /^(https?:\/\/|\/|#|\.\/|fms-app:\/\/wiki\/)/.test(s) ? s : "#";
}

// ============================================================
// Inline parser
// ============================================================

function parseInline(text: string): ReactNode[] {
    const result: ReactNode[] = [];
    let pos = 0;
    let textBuffer = "";
    let key = 0;

    const flush = () => {
        if (textBuffer) { result.push(textBuffer); textBuffer = ""; }
    };

    while (pos < text.length) {
        const rest = text.slice(pos);
        let m: RegExpMatchArray | null;

        // Image: ![alt](url) — must be checked before link
        if ((m = rest.match(/^!\[([^\]]*)\]\(([^)]+)\)/))) {
            flush();
            result.push(createElement("img", { key: key++, src: sanitizeUrl(m[2]), alt: m[1] }));
            pos += m[0].length;
            continue;
        }

        // Link: [text](url)
        if ((m = rest.match(/^\[([^\]]+)\]\(([^)]+)\)/))) {
            flush();
            result.push(createElement("a", { key: key++, href: sanitizeUrl(m[2]) }, ...parseInline(m[1])));
            pos += m[0].length;
            continue;
        }

        // Inline code: `code`
        if ((m = rest.match(/^`([^`\n]+)`/))) {
            flush();
            result.push(createElement("code", { key: key++ }, m[1]));
            pos += m[0].length;
            continue;
        }

        // Bold italic: ***...*** or ___...___
        if ((m = rest.match(/^\*\*\*([\s\S]+?)\*\*\*/)) || (m = rest.match(/^___([\s\S]+?)___/))) {
            flush();
            result.push(createElement("strong", { key: key++ },
                createElement("em", null, ...parseInline(m![1]))
            ));
            pos += m![0].length;
            continue;
        }

        // Bold: **...** or __...__
        if ((m = rest.match(/^\*\*([\s\S]+?)\*\*/)) || (m = rest.match(/^__([\s\S]+?)__/))) {
            flush();
            result.push(createElement("strong", { key: key++ }, ...parseInline(m![1])));
            pos += m![0].length;
            continue;
        }

        // Italic: *...* (not **)
        if ((m = rest.match(/^\*([^*\n]+)\*/))) {
            flush();
            result.push(createElement("em", { key: key++ }, ...parseInline(m[1])));
            pos += m[0].length;
            continue;
        }

        // Italic: _..._ (not __)
        if ((m = rest.match(/^_([^_\n]+)_/))) {
            flush();
            result.push(createElement("em", { key: key++ }, ...parseInline(m[1])));
            pos += m[0].length;
            continue;
        }

        // Strikethrough: ~~...~~
        if ((m = rest.match(/^~~([\s\S]+?)~~/))) {
            flush();
            result.push(createElement("del", { key: key++ }, ...parseInline(m[1])));
            pos += m[0].length;
            continue;
        }

        // Newline → <br> (mirrors marked's breaks:true behaviour)
        if (text[pos] === "\n") {
            flush();
            result.push(createElement("br", { key: key++ }));
            pos++;
            continue;
        }

        textBuffer += text[pos];
        pos++;
    }

    flush();
    return result;
}

// ============================================================
// Block parser
// ============================================================

function parseBlocks(text: string, keyStart = 0): ReactNode[] {
    const lines = text.split("\n");
    const result: ReactNode[] = [];
    let i = 0;
    let key = keyStart;

    while (i < lines.length) {
        const line = lines[i];
        const trimmed = line.trim();

        if (trimmed === "") { i++; continue; }

        // Fenced code block: ```[lang]
        const fenceMatch = trimmed.match(/^(`{3,})(.*)/);
        if (fenceMatch) {
            const fence = fenceMatch[1];
            const lang = fenceMatch[2].trim();
            const codeLines: string[] = [];
            i++;
            while (i < lines.length && !lines[i].trim().startsWith(fence)) {
                codeLines.push(lines[i]);
                i++;
            }
            i++; // skip closing fence
            result.push(
                createElement("pre", { key: key++ },
                    createElement("code", lang ? { className: `language-${lang}` } : {},
                        codeLines.join("\n")
                    )
                )
            );
            continue;
        }

        // Horizontal rule: --- / *** / ___
        if (/^(-{3,}|\*{3,}|_{3,})\s*$/.test(trimmed)) {
            result.push(createElement("hr", { key: key++ }));
            i++;
            continue;
        }

        // Blockquote
        if (/^> ?/.test(line)) {
            const bqLines: string[] = [];
            while (i < lines.length && /^> ?/.test(lines[i])) {
                bqLines.push(lines[i].replace(/^> ?/, ""));
                i++;
            }
            result.push(createElement("blockquote", { key: key++ }, ...parseBlocks(bqLines.join("\n"))));
            continue;
        }

        // Unordered list
        if (/^[-*+] /.test(trimmed)) {
            const items: string[] = [];
            while (i < lines.length) {
                const l = lines[i];
                if (/^[-*+] /.test(l)) {
                    items.push(l.replace(/^[-*+] /, ""));
                    i++;
                } else if (/^  /.test(l) && items.length > 0 && l.trim() !== "") {
                    items[items.length - 1] += "\n" + l.replace(/^  /, "");
                    i++;
                } else {
                    break;
                }
            }
            result.push(
                createElement("ul", { key: key++ },
                    ...items.map((item, idx) =>
                        createElement("li", { key: idx }, ...parseInline(item))
                    )
                )
            );
            continue;
        }

        // Ordered list
        if (/^\d+\. /.test(trimmed)) {
            const startNum = parseInt(trimmed.match(/^(\d+)/)![1]);
            const items: string[] = [];
            while (i < lines.length) {
                const l = lines[i];
                if (/^\d+\. /.test(l)) {
                    items.push(l.replace(/^\d+\. /, ""));
                    i++;
                } else if (/^   /.test(l) && items.length > 0 && l.trim() !== "") {
                    items[items.length - 1] += "\n" + l.replace(/^   /, "");
                    i++;
                } else {
                    break;
                }
            }
            const olProps: Record<string, unknown> = { key: key++ };
            if (startNum !== 1) olProps.start = startNum;
            result.push(
                createElement("ol", olProps,
                    ...items.map((item, idx) =>
                        createElement("li", { key: idx }, ...parseInline(item))
                    )
                )
            );
            continue;
        }

        // GFM table: first line has |, second line is a separator (|---|:---:|)
        if (line.includes("|") && i + 1 < lines.length && /^[\|:\-\s]+$/.test(lines[i + 1].trim())) {
            const parseRow = (l: string): string[] =>
                l.trim().replace(/^\||\|$/g, "").split("|").map(c => c.trim());

            const headerCells = parseRow(line);
            i += 2; // skip header + separator

            const bodyRows: string[][] = [];
            while (i < lines.length && lines[i].includes("|")) {
                bodyRows.push(parseRow(lines[i]));
                i++;
            }

            const hasHeaders = headerCells.some(h => h !== "");
            result.push(
                createElement("table", { key: key++ },
                    hasHeaders ? createElement("thead", null,
                        createElement("tr", null,
                            ...headerCells.map((h, j) =>
                                createElement("th", { key: j }, ...parseInline(h))
                            )
                        )
                    ) : null,
                    createElement("tbody", null,
                        ...bodyRows.map((row, ri) =>
                            createElement("tr", { key: ri },
                                ...row.map((cell, ci) =>
                                    createElement("td", { key: ci }, ...parseInline(cell))
                                )
                            )
                        )
                    )
                )
            );
            continue;
        }

        // Paragraph: collect consecutive non-block lines
        const paraLines: string[] = [];
        while (i < lines.length) {
            const l = lines[i];
            if (l.trim() === "") break;
            if (/^> ?/.test(l) || /^[-*+] /.test(l) || /^\d+\. /.test(l)) break;
            if (l.trim().match(/^`{3,}/)) break;
            if (/^(-{3,}|\*{3,}|_{3,})\s*$/.test(l.trim())) break;
            if (l.includes("|") && i + 1 < lines.length && /^[\|:\-\s]+$/.test(lines[i + 1]?.trim() ?? "")) break;
            paraLines.push(l);
            i++;
        }
        if (paraLines.length > 0) {
            result.push(createElement("p", { key: key++ }, ...parseInline(paraLines.join("\n"))));
        }
    }

    return result;
}

// ============================================================
// Main export
// ============================================================

export function toHTML(content: string): { toc: ReactNode; body: ReactNode[] } {
    const toc = new TOC();
    const bodyElements: ReactNode[] = [];
    let buffer = "";
    let inCodeBlock = false;

    const flushBuffer = () => {
        if (buffer.trim() !== "") {
            for (const node of parseBlocks(buffer, bodyElements.length)) bodyElements.push(node);
        }
        buffer = "";
    };

    const lines = content.replace(/\r/g, "").split("\n");
    const regHead = /^(#+)\s+(.*)$/;

    for (const line of lines) {
        // Track fenced code blocks so headings inside them are not parsed.
        if (line.trim().match(/^`{3,}/)) {
            inCodeBlock = !inCodeBlock;
            buffer += line + "\n";
            continue;
        }

        if (inCodeBlock) {
            buffer += line + "\n";
            continue;
        }

        if (regHead.test(line)) {
            flushBuffer();
            const [, hashes, title] = line.match(regHead)!;
            const depth = hashes.length;
            const sec = toc.newSection(depth);
            toc.add(title, sec, false);
            const level = Math.min(depth + 1, 6) as 1 | 2 | 3 | 4 | 5 | 6;
            bodyElements.push(
                createElement(`h${level}`, { key: `h-${bodyElements.length}`, id: `sec-${sec.toString()}` },
                    createElement("span", { className: `section-number-${level}` }, sec.toString()),
                    " ",
                    ...parseInline(title)
                )
            );
        } else {
            buffer += line + "\n";
        }
    }

    flushBuffer();

    return { toc: toc.react(), body: bodyElements };
}
