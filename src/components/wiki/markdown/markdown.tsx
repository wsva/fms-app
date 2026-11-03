"use client";

import { useEffect, useState, useCallback, type ReactNode } from "react";
import { toHTML } from "./html";
import "./markdown.css";

const WIKI_LINK_PREFIX = "fms-app://wiki/";

// A relative link to another markdown file, with an optional #anchor: the
// style the manual / README are written in ("foo.md", "./foo.md",
// "../dir/foo.md", "/foo.md"). Rejects anything carrying a URL scheme so an
// external http(s)://…md link still opens in the browser.
const isRelativeMdLink = (href: string) =>
    !/^[a-z][a-z0-9+.\-]*:/i.test(href) && !href.startsWith("//") && /\.md(#[^#]*)?$/i.test(href);

type Props = {
    content?: string;
    // Renders the TOC inline above the body. The wiki page and the card viewer
    // both leave this false and show the same TOC in their own left drawer
    // instead (they build the node via toHTML).
    withTOC?: boolean;
    onWikiLink?: (relativePath: string) => void;
}

export default function MarkdownViewer({ content, withTOC = false, onWikiLink }: Props) {
    const [stateContent, setStateContent] = useState<string>(content || "");
    const [stateTOC, setStateTOC] = useState<ReactNode>(null);
    const [stateBody, setStateBody] = useState<ReactNode[]>([]);

    useEffect(() => {
        if (content !== undefined) setStateContent(content);
    }, [content]);

    useEffect(() => {
        const { toc, body } = toHTML(stateContent);
        setStateTOC(toc);
        setStateBody(body);
    }, [stateContent]);

    // Intercept clicks on internal links for in-page navigation:
    //  - fms-app://wiki/... links, whose tail is the dataset-root-relative path;
    //  - relative .md links as written in ordinary markdown (manual, README),
    //    passed through as-is so the embedding page resolves them against the
    //    current document's directory;
    //  - #anchor jumps, scrolled within the rendered body instead of navigating.
    const handleClick = useCallback((e: React.MouseEvent) => {
        const target = (e.target as HTMLElement).closest("a");
        if (!target) return;
        const href = target.getAttribute("href") || "";
        if (href.startsWith("#")) {
            e.preventDefault();
            const el = document.getElementById(decodeURIComponent(href.slice(1)));
            el?.scrollIntoView({ behavior: "smooth", block: "start" });
            return;
        }
        if (!onWikiLink) return;
        if (href.startsWith(WIKI_LINK_PREFIX)) {
            e.preventDefault();
            const relativePath = href.slice(WIKI_LINK_PREFIX.length);
            if (relativePath) onWikiLink(relativePath);
        } else if (isRelativeMdLink(href)) {
            e.preventDefault();
            onWikiLink(href);
        }
    }, [onWikiLink]);

    return (
        <div className="md-container" onClick={handleClick}>
            {(withTOC && !!stateTOC) && stateTOC}
            <article className="md-body">
                {stateBody}
            </article>
        </div>
    );
}
