import { type ClampLines, clampClass } from "./clamp";

/** The body sizes Markdown text takes: 16 for a lead summary, 15 for body text, 14 for side text, 13 for captions. */
export type MarkdownSize = 13 | 14 | 15 | 16;

const SIZES: Record<MarkdownSize, string> = {
  13: "text-13",
  14: "text-14",
  15: "text-15",
  16: "text-16",
};

/** A heading stays in the size of its text and stands out by weight; only body text gives its first heading one step more. */
const HEADINGS: Record<MarkdownSize, string> = {
  13: "text-13",
  14: "text-14",
  15: "text-16",
  16: "text-16",
};

export function markdownSizeClass(size: MarkdownSize): string {
  return SIZES[size];
}

export function headingSizeClass(size: MarkdownSize, level: number): string {
  return level === 1 ? HEADINGS[size] : SIZES[size];
}

/**
 * The box around the blocks. Open, the blocks sit in a column with a gap. Clamped, the box is cut to its first lines
 * with an ellipsis: a line clamp counts the lines of the blocks the box holds itself, so it cannot be a flex column, and
 * margins keep the blocks apart instead.
 */
export function markdownBoxClass(clamp: ClampLines | null): string {
  return clamp === null
    ? "flex min-w-0 flex-col gap-6"
    : `min-w-0 [&>*+*]:mt-6 ${clampClass(clamp, false)}`;
}

/** How a link goes: `app` to a screen of the app through the router, `away` to another site, `page` to an address relative to the page, and `none` when the renderer emptied an unsafe address, which shows as text. */
export type LinkKind = "app" | "away" | "page" | "none";

/** A browser reads `//host` and `/\host` as another site, so only a `/` with neither after it stays in the app. */
export function linkKind(href: string | undefined): LinkKind {
  if (href === undefined || href === "") return "none";
  if (/^(https?:|mailto:|[/\\]{2})/i.test(href)) return "away";
  if (href.startsWith("/")) return "app";
  return "page";
}

/** A link that leaves the app opens in a new tab and tells the other site nothing of the page it came from. */
export function linkTarget(
  href: string | undefined,
): { target: "_blank"; rel: "noopener noreferrer" } | null {
  return linkKind(href) === "away"
    ? { target: "_blank", rel: "noopener noreferrer" }
    : null;
}

/** Longer text shows as plain text: Markdown would take too long to parse and draw it. */
export const MAX_MARKDOWN_CHARS = 100_000;
/** Deeper quotes and lists show as plain text: each level is one more element, and a deep enough tree overflows the renderer's stack. */
export const MAX_MARKDOWN_DEPTH = 100;

const CONTAINER_MARK = /(?:>|[-*+](?=\s)|\d{1,9}[.)](?=\s)|[ \t]+)/y;

/** How deep the blocks at the start of a line nest: each quote mark and list mark is a level, and two columns of indent are one. */
function lineDepth(line: string): number {
  let depth = 0;
  let indent = 0;
  CONTAINER_MARK.lastIndex = 0;
  for (;;) {
    const mark = CONTAINER_MARK.exec(line)?.[0];
    if (mark === undefined) break;
    if (mark.trim() === "") {
      indent += mark.replace(/\t/g, "    ").length;
    } else {
      depth += 1;
    }
  }
  return depth + Math.floor(indent / 2);
}

/** Whether `source` shows as plain text instead of Markdown: it is too long, or its quotes and lists nest too deep. Lines inside a code fence do not count. */
export function rendersAsPlain(source: string): boolean {
  if (source.length > MAX_MARKDOWN_CHARS) return true;
  let fenced = false;
  for (const line of source.split("\n")) {
    if (/^\s*(```|~~~)/.test(line)) {
      fenced = !fenced;
      continue;
    }
    if (!fenced && lineDepth(line) > MAX_MARKDOWN_DEPTH) return true;
  }
  return false;
}

/** The part of an mdast node the break plugin reads and writes. */
interface BreakNode {
  type: string;
  value?: string;
  children?: BreakNode[];
}

/** Code spans, code blocks and raw HTML are not text nodes, so their line ends stay as they are. */
function splitLines(node: BreakNode): BreakNode[] {
  const lines = (node.value ?? "").split("\n");
  return lines.flatMap((line, index) => {
    const parts: BreakNode[] = index > 0 ? [{ type: "break" }] : [];
    if (line !== "") parts.push({ type: "text", value: line });
    return parts;
  });
}

function walk(node: BreakNode): void {
  if (!node.children) return;
  node.children = node.children.flatMap((child) => {
    if (child.type === "text" && child.value?.includes("\n")) {
      return splitLines(child);
    }
    walk(child);
    return [child];
  });
}

/**
 * A remark plugin that keeps each line end inside a paragraph as a line break. Agents and people write texts with one
 * fact on each line, and Markdown would join those lines into one run.
 */
export function remarkLineBreaks() {
  return (tree: BreakNode) => {
    walk(tree);
  };
}

/** Blocks that hold other blocks: raw HTML directly inside one is a block of its own and becomes a paragraph. */
const BLOCK_PARENTS = new Set(["root", "blockquote", "listItem"]);

function htmlToText(node: BreakNode): void {
  if (!node.children) return;
  const block = BLOCK_PARENTS.has(node.type);
  node.children = node.children.map((child) => {
    if (child.type !== "html") {
      htmlToText(child);
      return child;
    }
    const text: BreakNode = { type: "text", value: child.value ?? "" };
    return block ? { type: "paragraph", children: [text] } : text;
  });
}

/**
 * A remark plugin that shows raw HTML as the text it was typed as, so a message that names `<Button>` without
 * backticks keeps the name. React escapes the text, so no element is ever made from it.
 */
export function remarkHtmlAsText() {
  return (tree: BreakNode) => {
    htmlToText(tree);
  };
}
