import { parseId, type IdKind } from "@/format/ids";

/** `#T-12`: a fold item by its public id. */
export interface ItemMention {
  mark: "#";
  kind: IdKind;
  /** The id without the mark, for example `T-12`. */
  id: string;
  /** The mention as written, mark included. */
  text: string;
  start: number;
  end: number;
}

/** `@web/src/board.ts` names a file in the main tree; `@ws-9:web/src/board.ts` names one in a workspace. */
export interface PathMention {
  mark: "@";
  workspace: string | null;
  path: string;
  text: string;
  start: number;
  end: number;
}

export type Mention = ItemMention | PathMention;

export interface Segment {
  text: string;
  mention?: Mention;
}

/** Decides whether a well-formed mention names something that exists; `true` when the caller cannot tell. */
export type Known = (mention: Mention) => boolean;

// A mark counts only at the start of a word, so `page#T-1`, `a@b.c` and `&#12;` stay text.
const ITEM = /(?<![\w#&/@])#([A-Za-z]{1,3}-?[0-9]+)(?![\w]|-[\w])/g;
const PATH = /(?<![\w@./#])@(?:(ws-[1-9][0-9]{0,9}):)?([\w.\-/]+)/g;

/** A relative, normalised path: no leading or doubled `/`, no `.` or `..` segment. */
export function isCleanPath(path: string): boolean {
  if (path === "") return false;
  return path
    .split("/")
    .every((segment) => segment !== "" && segment !== "." && segment !== "..");
}

/** Sentence punctuation after a path is not part of it: `see @a/b.ts.` names `a/b.ts`. */
function trimPath(raw: string): string {
  return raw.replace(/[.-]+$/, "");
}

function itemMentions(text: string): ItemMention[] {
  const found: ItemMention[] = [];
  for (const match of text.matchAll(ITEM)) {
    const raw = match[1] ?? "";
    const id = parseId(raw);
    if (id === null) continue;
    found.push({
      mark: "#",
      kind: id.kind,
      id: raw,
      text: match[0],
      start: match.index,
      end: match.index + match[0].length,
    });
  }
  return found;
}

/**
 * A main-tree path needs a `/` or a `.` (`@web/src`, `@Makefile.am`), so a word after `@` such as `@tanim` stays text; a workspace path always counts.
 */
function pathMentions(text: string): PathMention[] {
  const found: PathMention[] = [];
  for (const match of text.matchAll(PATH)) {
    const workspace = match[1] ?? null;
    const path = trimPath(match[2] ?? "");
    if (!isCleanPath(path)) continue;
    if (workspace === null && !/[./]/.test(path)) continue;
    const written = `@${workspace === null ? "" : `${workspace}:`}${path}`;
    found.push({
      mark: "@",
      workspace,
      path,
      text: written,
      start: match.index,
      end: match.index + written.length,
    });
  }
  return found;
}

/** Every well-formed mention in `text`, in order. Code spans are the caller's: Markdown hands over text nodes only, and `splitMentions` skips them. */
export function findMentions(text: string): Mention[] {
  return [...itemMentions(text), ...pathMentions(text)].sort(
    (a, b) => a.start - b.start,
  );
}

/** The spans of inline code: a run of backticks up to the next run of the same length. A run without a partner is text, as in Markdown. */
export function codeRanges(text: string): [number, number][] {
  const runs = [...text.matchAll(/`+/g)].map(
    (m) => [m.index, m[0].length] as const,
  );
  const ranges: [number, number][] = [];
  let i = 0;
  while (i < runs.length) {
    const open = runs[i];
    if (open === undefined) break;
    const closeAt = runs.findIndex((run, j) => j > i && run[1] === open[1]);
    const close = runs[closeAt];
    if (close === undefined) {
      i += 1;
      continue;
    }
    ranges.push([open[0], close[0] + close[1]]);
    i = closeAt + 1;
  }
  return ranges;
}

export function inCode(ranges: readonly [number, number][], at: number) {
  return ranges.some(([from, to]) => at >= from && at < to);
}

/** Splits plain text into text and mentions. A mention inside a code span, or one `known` rejects, stays text. */
export function splitMentions(text: string, known?: Known): Segment[] {
  const code = codeRanges(text);
  const segments: Segment[] = [];
  let at = 0;
  for (const mention of findMentions(text)) {
    if (inCode(code, mention.start) || (known && !known(mention))) continue;
    if (mention.start > at)
      segments.push({ text: text.slice(at, mention.start) });
    segments.push({ text: mention.text, mention });
    at = mention.end;
  }
  if (at < text.length) segments.push({ text: text.slice(at) });
  return segments;
}

/** The screen a mention opens. */
export function mentionHref(slug: string, mention: Mention): string {
  const base = `/p/${slug}`;
  if (mention.mark === "@") {
    const query = new URLSearchParams(
      mention.workspace === null
        ? { path: mention.path }
        : { to: mention.workspace, path: mention.path },
    ).toString();
    return mention.workspace === null
      ? `${base}/repository?${query}`
      : `${base}/diff?${query}`;
  }
  const id = mention.id;
  switch (mention.kind) {
    case "task":
      return `${base}/t/${id}`;
    case "proposal":
      return `${base}/proposals/${id}`;
    case "request":
      return `${base}/requests/${id}`;
    case "revision":
      return `${base}/repository?revision=${id}`;
    case "message":
      return `${base}/messages?thread=${id}`;
    case "changeset":
    case "workspace":
    case "checkpoint":
      return `${base}/diff?to=${id}`;
  }
}

/** The ids of the lists a screen holds; a list it does not hold leaves its kind unknown, so those mentions stay links. */
export interface KnownIds {
  proposals?: readonly string[];
  requests?: readonly string[];
}

export function knownFrom(ids: KnownIds): Known {
  return (mention) => {
    if (mention.mark === "@") return true;
    const list =
      mention.kind === "proposal"
        ? ids.proposals
        : mention.kind === "request"
          ? ids.requests
          : undefined;
    return list === undefined || list.includes(mention.id);
  };
}
