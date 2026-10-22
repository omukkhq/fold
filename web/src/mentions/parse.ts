import { parseId, type IdKind } from "@/format/ids";
import {
  NO_PEOPLE,
  personOf,
  type People,
  type PersonKind,
} from "@/people/names";
import { idHref, projectPath, withQuery } from "@/routes/paths";

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

/** `@tanim` or `@frontend`: a human or an agent by a name the daemon knows. */
export interface PersonMention {
  mark: "@";
  person: PersonKind;
  /** The name without the mark and without `human:`. */
  name: string;
  text: string;
  start: number;
  end: number;
}

/** `@web/src/board.ts` names a file in the main tree; `@ws-9:web/src/board.ts` names one in a workspace; a trailing `/` (`@web/src/`) or `@./name` says which of the two it is. */
export interface PathMention {
  mark: "@";
  workspace: string | null;
  /** Without a trailing `/`, also for a folder. */
  path: string;
  text: string;
  start: number;
  end: number;
}

export type Mention = ItemMention | PersonMention | PathMention;

export function isPerson(mention: Mention): mention is PersonMention {
  return mention.mark === "@" && "person" in mention;
}

export function isPath(mention: Mention): mention is PathMention {
  return mention.mark === "@" && "path" in mention;
}

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
 * What follows `@` is a person when it is a name in `people` and does not end with `/`; otherwise a folder when it ends with `/`, otherwise a file. A main-tree file needs a `/` or a `.` (`@web/src`, `@Makefile.am`), so an unknown word such as `@nobody` stays text; `@./Makefile` forces a file and a workspace path always counts.
 */
function atMentions(
  text: string,
  people: People,
): (PersonMention | PathMention)[] {
  const found: (PersonMention | PathMention)[] = [];
  for (const match of text.matchAll(PATH)) {
    const workspace = match[1] ?? null;
    const raw = trimPath(match[2] ?? "");
    const forced = workspace === null && raw.startsWith("./");
    const spelled = forced ? raw.slice(2) : raw;
    const folder = spelled.endsWith("/");
    const path = folder ? spelled.slice(0, -1) : spelled;
    const prefix = workspace === null ? "" : `${workspace}:`;
    const index = match.index;
    if (workspace === null && !forced) {
      const person = personOf(people, raw);
      if (person !== null) {
        found.push({
          mark: "@",
          person,
          name: raw,
          text: `@${raw}`,
          start: index,
          end: index + raw.length + 1,
        });
        continue;
      }
    }
    if (!isCleanPath(path)) continue;
    if (workspace === null && !forced && !/[./]/.test(spelled)) continue;
    const written = `@${forced ? "./" : prefix}${path}${folder ? "/" : ""}`;
    found.push({
      mark: "@",
      workspace,
      path,
      text: written,
      start: index,
      end: index + written.length,
    });
  }
  return found;
}

/** Every well-formed mention in `text`, in order. Code spans are the caller's: Markdown hands over text nodes only, and `splitMentions` skips them. */
export function findMentions(
  text: string,
  people: People = NO_PEOPLE,
): Mention[] {
  return [...itemMentions(text), ...atMentions(text, people)].sort(
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
export function splitMentions(
  text: string,
  known?: Known,
  people: People = NO_PEOPLE,
): Segment[] {
  const code = codeRanges(text);
  const segments: Segment[] = [];
  let at = 0;
  for (const mention of findMentions(text, people)) {
    if (inCode(code, mention.start) || (known && !known(mention))) continue;
    if (mention.start > at)
      segments.push({ text: text.slice(at, mention.start) });
    segments.push({ text: mention.text, mention });
    at = mention.end;
  }
  if (at < text.length) segments.push({ text: text.slice(at) });
  return segments;
}

/** The screen a mention opens; a human has none. */
export function mentionHref(slug: string, mention: Mention): string | null {
  if (isPerson(mention)) {
    // The board filters by agent, so a worker such as `frontend/ui` opens its agent's cards.
    return mention.person === "agent"
      ? withQuery(projectPath(slug, "board"), {
          agent: mention.name.split("/")[0] ?? mention.name,
        })
      : null;
  }
  if (mention.mark === "@") {
    const query: Record<string, string> =
      mention.workspace === null
        ? { path: mention.path }
        : { to: mention.workspace, path: mention.path };
    return withQuery(
      projectPath(slug, mention.workspace === null ? "repository" : "diff"),
      query,
    );
  }
  return idHref(slug, mention.kind, mention.id);
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
