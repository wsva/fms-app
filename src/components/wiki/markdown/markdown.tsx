"use client";

import { useEffect, useState, useCallback, type ReactNode } from "react";
import { toHTML } from "./html";
import "./markdown.css";

const WIKI_LINK_PREFIX = "fms-app://wiki/";

type Props = {
    content?: string;
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

    // Intercept clicks on fms-app://wiki/ links for internal navigation
    const handleClick = useCallback((e: React.MouseEvent) => {
        if (!onWikiLink) return;
        const target = (e.target as HTMLElement).closest("a");
        if (!target) return;
        const href = target.getAttribute("href");
        if (href && href.startsWith(WIKI_LINK_PREFIX)) {
            e.preventDefault();
            const relativePath = href.slice(WIKI_LINK_PREFIX.length);
            if (relativePath) onWikiLink(relativePath);
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
