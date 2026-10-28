import type { Hunk } from "@/api/diff";
import type { ReviewComment } from "@/api/proposals";
import {
  fileLineSet,
  gapText,
  type FileTextView,
  type LineSet,
} from "@/features/diff/gaps";
import type { DiffSelection, DiffThread } from "@/features/diff/selection";

export interface ThreadComment {
  comment: ReviewComment;
  /** A reply to an earlier comment of the thread. */
  reply: boolean;
}

/** The comments on one range of one file, or the comments on the version as a whole, with replies under the comment they answer. */
export interface CommentThread {
  /** The id of the thread's first comment. */
  id: string;
  /** Null for comments on the version as a whole. */
  range: DiffSelection | null;
  /** The version the thread's first comment was written on. */
  version: number;
  /** Written on an earlier version than the one shown. */
  outdated: boolean;
  comments: ThreadComment[];
}

function rangeOf(c: ReviewComment): DiffSelection | null {
  if (
    c.path === null ||
    c.side === null ||
    c.start_line === null ||
    c.end_line === null
  ) {
    return null;
  }
  return {
    path: c.path,
    side: c.side,
    start_line: c.start_line,
    end_line: c.end_line,
  };
}

function rangeKey(version: number, range: DiffSelection | null): string {
  if (range === null) return `${String(version)}|general`;
  return [
    String(version),
    range.path,
    range.side,
    String(range.start_line),
    String(range.end_line),
  ].join("|");
}

/** The first comment a reply chain goes back to. A missing parent ends the chain; a chain that loops ends at its earliest comment, so every comment of the loop finds the same root. */
function rootOf(
  c: ReviewComment,
  byId: ReadonlyMap<string, ReviewComment>,
): ReviewComment {
  const chain = [c];
  let current = c;
  while (current.reply_to !== null) {
    const parent = byId.get(current.reply_to);
    if (!parent) return current;
    if (chain.includes(parent)) {
      return chain.reduce((a, b) => (b.created_at < a.created_at ? b : a));
    }
    chain.push(parent);
    current = parent;
  }
  return current;
}

/** Each comment after its parent; siblings in time order. */
function nest(
  comments: readonly ReviewComment[],
  rootIds: ReadonlySet<string>,
): ThreadComment[] {
  const children = new Map<string | null, ReviewComment[]>();
  const ids = new Set(comments.map((c) => c.id));
  for (const c of comments) {
    const parent =
      rootIds.has(c.id) || c.reply_to === null || !ids.has(c.reply_to)
        ? null
        : c.reply_to;
    const list = children.get(parent) ?? [];
    list.push(c);
    children.set(parent, list);
  }
  const out: ThreadComment[] = [];
  const visit = (parent: string | null) => {
    const list = [...(children.get(parent) ?? [])].sort(
      (a, b) => a.created_at - b.created_at,
    );
    for (const c of list) {
      out.push({ comment: c, reply: parent !== null });
      visit(c.id);
    }
  };
  visit(null);
  return out;
}

/**
 * The threads of the version on screen. A thread starts at a comment that replies to nothing; replies join the thread of the comment they answer, whatever version they were written on.
 * Starting comments on the same range of the same version share one thread. Threads from later versions than the one shown are left out, and threads from earlier ones are outdated.
 */
export function buildThreads(
  comments: readonly ReviewComment[],
  shownVersion: number,
): CommentThread[] {
  const byId = new Map(comments.map((c) => [c.id, c]));
  const groups = new Map<string, ReviewComment[]>();
  const rootIds = new Set<string>();
  const order: string[] = [];
  const keyOfRoot = new Map<string, string>();
  for (const c of comments) {
    const root = rootOf(c, byId);
    if (root.version > shownVersion) continue;
    let key = keyOfRoot.get(root.id);
    if (key === undefined) {
      key = rangeKey(root.version, rangeOf(root));
      keyOfRoot.set(root.id, key);
    }
    if (root.id === c.id) rootIds.add(c.id);
    const group = groups.get(key);
    if (group) {
      group.push(c);
    } else {
      groups.set(key, [c]);
      order.push(key);
    }
  }
  return order.flatMap((key) => {
    const group = groups.get(key) ?? [];
    const first = group.find((c) => rootIds.has(c.id));
    if (!first) return [];
    const thread: CommentThread = {
      id: first.id,
      range: rangeOf(first),
      version: first.version,
      outdated: first.version < shownVersion,
      comments: nest(group, rootIds),
    };
    return [thread];
  });
}

export interface FileThread {
  thread: CommentThread;
  range: DiffSelection;
}

export interface FileThreads {
  path: string;
  threads: FileThread[];
}

/** Threads that sit on lines of a file, by path, each file's threads in line order. */
export function groupByFile(threads: readonly CommentThread[]): FileThreads[] {
  const files = new Map<string, FileThread[]>();
  for (const thread of threads) {
    const range = thread.range;
    if (range === null) continue;
    const list = files.get(range.path) ?? [];
    list.push({ thread, range });
    files.set(range.path, list);
  }
  return [...files.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([path, list]) => ({
      path,
      threads: [...list].sort(
        (a, b) =>
          a.range.start_line - b.range.start_line ||
          a.range.end_line - b.range.end_line,
      ),
    }));
}

/** The anchors the diff viewer draws threads at. */
export function diffThreads(threads: readonly CommentThread[]): DiffThread[] {
  return groupByFile(threads).flatMap((file) =>
    file.threads.map(({ thread, range }) => ({ id: thread.id, range })),
  );
}

/** Threads the diff cannot place: comments on the version as a whole and comments on files the comparison does not list. */
export function unplacedThreads(
  threads: readonly CommentThread[],
  listedPaths: readonly string[],
): CommentThread[] {
  const listed = new Set(listedPaths);
  return threads.filter((t) => t.range === null || !listed.has(t.range.path));
}

export interface PlacedThreads {
  /** The anchors the diff viewer draws threads at. */
  placed: DiffThread[];
  /** The threads listed under the diff. */
  unplaced: CommentThread[];
}

/** The lines a file's hunks hold, as `side:line` keys. A context line holds both its old and its new number. */
export function heldLines(hunks: readonly Hunk[]): Set<string> {
  const held = new Set<string>();
  for (const hunk of hunks) {
    for (const line of hunk.lines) {
      if (line.old_no !== null) held.add(`old:${String(line.old_no)}`);
      if (line.new_no !== null) held.add(`new:${String(line.new_no)}`);
    }
  }
  return held;
}

/**
 * The lines a thread can sit under in a file whose hunks are read: every line of both sides once the file's text fits the hunks, as the gaps
 * around them can show each one; the hunks' lines when the file has no text to read or its read failed; null while the text loads, as nothing
 * is known yet.
 */
export function heldFileLines(
  hunks: readonly Hunk[],
  newHash: string | null,
  fileText: FileTextView | undefined,
): LineSet | null {
  if (fileText === undefined || hunks.length === 0) return heldLines(hunks);
  const text = gapText(hunks, newHash, fileText);
  if (text?.status === "pending") return null;
  if (text?.status === "ready") return fileLineSet(hunks, text.lines.length);
  return heldLines(hunks);
}

/**
 * Places threads on a diff of `listedPaths`. Without the old side, as on a change since an earlier version whose old lines no comment reads, a thread on the old side joins the threads the diff cannot place.
 * `held` names the lines the hunks of the files already read hold: a thread whose end is on no such line has no row to sit under and joins the threads the diff cannot place. A listed file with no entry in `held` has not been read, so its threads stay with the diff.
 */
export function placeThreads(
  threads: readonly CommentThread[],
  listedPaths: readonly string[],
  oldSide: boolean,
  held: ReadonlyMap<string, LineSet> = new Map(),
): PlacedThreads {
  const listed = new Set(listedPaths);
  const onDiff = (t: CommentThread) => {
    const range = t.range;
    if (range === null || !listed.has(range.path)) return false;
    if (!oldSide && range.side !== "new") return false;
    const lines = held.get(range.path);
    return (
      lines === undefined ||
      lines.has(`${range.side}:${String(range.end_line)}`)
    );
  };
  return {
    placed: diffThreads(threads.filter(onDiff)),
    unplaced: threads.filter((t) => !onDiff(t)),
  };
}

export function linesText(range: DiffSelection): string {
  return range.start_line === range.end_line
    ? `line ${String(range.start_line)}`
    : `lines ${String(range.start_line)}–${String(range.end_line)}`;
}

/** The note beside the author: `on v1 · line 10 · outdated` for a thread's first comment, `reply · v2` for a reply. */
export function commentNote(
  thread: CommentThread,
  entry: ThreadComment,
): string {
  if (entry.reply) return `reply · v${String(entry.comment.version)}`;
  const parts = [`on v${String(thread.version)}`];
  if (thread.range !== null) parts.push(linesText(thread.range));
  if (thread.outdated) parts.push("outdated");
  return parts.join(" · ");
}

/** The composer's head: `Comment on lines 2–4 · new side · tanim`. */
export function composerTitle(selection: DiffSelection, human: string): string {
  const lines = linesText(selection);
  return `Comment on ${lines} · ${selection.side} side · ${human}`;
}
