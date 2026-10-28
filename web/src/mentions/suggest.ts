import type { People } from "@/people/names";
import { codeRanges, inCode } from "./parse";

export type Mark = "#" | "@";

/** The mention being typed at the caret: its mark, what follows it, and the span a pick replaces. */
export interface ActiveMention {
  mark: Mark;
  query: string;
  start: number;
  end: number;
}

const TOKEN = /[\w.\-/:]/;

/** The mention the caret is in, or null. A mark counts at the start of a word and outside code spans. */
export function activeMention(
  text: string,
  caret: number,
): ActiveMention | null {
  let start = caret;
  while (start > 0 && TOKEN.test(text.charAt(start - 1))) start -= 1;
  const markAt = start - 1;
  const mark = text.charAt(markAt);
  if (mark !== "#" && mark !== "@") return null;
  const before = text.charAt(markAt - 1);
  if (markAt > 0 && /[\w#@&/.]/.test(before)) return null;
  if (inCode(codeRanges(text), markAt)) return null;
  let end = caret;
  while (end < text.length && TOKEN.test(text.charAt(end))) end += 1;
  return { mark, query: text.slice(start, caret), start: markAt, end };
}

export interface Picked {
  text: string;
  caret: number;
}

/** Replaces the mention with the pick. A finished pick gets a space after it; a directory stays open for the next part. */
export function applyPick(
  text: string,
  active: ActiveMention,
  insert: string,
  open: boolean,
): Picked {
  const after = text.slice(active.end);
  const head = text.slice(0, active.start) + active.mark + insert;
  if (open) return { text: head + after, caret: head.length };
  const space = /^\s/.test(after) ? "" : " ";
  return { text: head + space + after, caret: head.length + 1 };
}

export type Group =
  | "people"
  | "tasks"
  | "proposals"
  | "requests"
  | "revisions"
  | "files";

export const GROUP_LABELS: Record<Group, string> = {
  people: "People",
  tasks: "Tasks",
  proposals: "Proposals",
  requests: "Requests",
  revisions: "Revisions",
  files: "Files",
};

export interface Suggestion {
  group: Group;
  /** What the pick writes after the mark. */
  insert: string;
  /** The id, path or name the row shows: in the mono face, except a person's name. */
  label: string;
  /** The title beside it; empty when the item has none. */
  detail: string;
  /** A directory: the pick keeps the popover open. */
  open: boolean;
}

export interface Item {
  id: string;
  title: string;
}

export interface ItemSources {
  tasks?: readonly Item[];
  proposals?: readonly Item[];
  requests?: readonly Item[];
  revisions?: readonly Item[];
}

const PER_GROUP = 5;
const ITEM_GROUPS = ["tasks", "proposals", "requests", "revisions"] as const;

function matches(item: Item, query: string): boolean {
  const q = query.toLowerCase();
  return (
    item.id.toLowerCase().startsWith(q) || item.title.toLowerCase().includes(q)
  );
}

/** Items whose id starts with the query or whose title holds it, at most five per group, in the order each source lists them. */
export function itemSuggestions(
  query: string,
  sources: ItemSources,
): Suggestion[] {
  return ITEM_GROUPS.flatMap((group) =>
    (sources[group] ?? [])
      .filter((item) => matches(item, query))
      .slice(0, PER_GROUP)
      .map((item) => ({
        group,
        insert: item.id,
        label: item.id,
        detail: item.title,
        open: false,
      })),
  );
}

export type RowIcon = "user" | "bot" | "file" | "folder";

/** The icon a People or Files row starts with; item rows have none. */
export function rowIcon(suggestion: Suggestion): RowIcon | null {
  if (suggestion.group === "people") {
    return suggestion.detail === "human" ? "user" : "bot";
  }
  if (suggestion.group === "files") return suggestion.open ? "folder" : "file";
  return null;
}

/** The humans, then the agents and workers, whose name starts with the typed part, at most five per kind. A name is written as it is, without `human:`. */
export function personSuggestions(query: string, people: People): Suggestion[] {
  const q = query.toLowerCase();
  const pick = (names: readonly string[], detail: string) =>
    names
      .filter((name) => name.toLowerCase().startsWith(q))
      .slice(0, PER_GROUP)
      .map((name) => ({
        group: "people" as const,
        insert: name,
        label: name,
        detail,
        open: false,
      }));
  return [...pick(people.humans, "human"), ...pick(people.agents, "agent")];
}

/** How a row shows its label: an id or a path in the mono face, a person as `@name` in Barlow in the colour of their kind. */
export type OptionLook = "path" | "id" | "human" | "agent";

/** A file path in the mono face, an item's id in the body face as every id reads, and a person in the colour of their kind. */
export function optionLook(suggestion: Suggestion): OptionLook {
  if (suggestion.group === "files") return "path";
  if (suggestion.group !== "people") return "id";
  return suggestion.detail === "human" ? "human" : "agent";
}

/** The mark a row shows before its label. */
export function optionMark(group: Group): Mark {
  return group === "files" || group === "people" ? "@" : "#";
}

export interface PathQuery {
  workspace: string | null;
  /** The directory to list, `""` for the root. */
  dir: string;
  /** The start of the name inside `dir`. */
  leaf: string;
}

/** Splits `@web/src/bo` into the directory to list and the name being typed; null for a path that climbs or doubles a `/`. */
export function pathQuery(query: string): PathQuery | null {
  const colon = /^(ws-[1-9][0-9]*):(.*)$/.exec(query);
  const workspace = colon ? (colon[1] ?? null) : null;
  const rest = colon ? (colon[2] ?? "") : query;
  const slash = rest.lastIndexOf("/");
  const dir = slash < 0 ? "" : rest.slice(0, slash);
  const leaf = slash < 0 ? rest : rest.slice(slash + 1);
  if (
    dir !== "" &&
    dir.split("/").some((s) => s === "" || s === "." || s === "..")
  ) {
    return null;
  }
  return { workspace, dir, leaf };
}

export interface Entry {
  name: string;
  path: string;
  kind: string;
}

/** The entries of the listed directory whose name starts with the typed part; a directory writes its path with a `/` and keeps the popover open. */
export function pathSuggestions(
  query: PathQuery,
  entries: readonly Entry[],
): Suggestion[] {
  const leaf = query.leaf.toLowerCase();
  const prefix = query.workspace === null ? "" : `${query.workspace}:`;
  return entries
    .filter((entry) => entry.name.toLowerCase().startsWith(leaf))
    .slice(0, PER_GROUP * 2)
    .map((entry) => {
      const directory = entry.kind === "directory";
      return {
        group: "files" as const,
        insert: `${prefix}${entry.path}${directory ? "/" : ""}`,
        label: `${entry.name}${directory ? "/" : ""}`,
        detail: entry.path,
        open: directory,
      };
    });
}

/** The highlighted row after an arrow key; it wraps at both ends. */
export function stepIndex(
  index: number,
  count: number,
  direction: 1 | -1,
): number {
  if (count === 0) return 0;
  return (index + direction + count) % count;
}

/** Consecutive suggestions of one group, for the group headings. */
export function groupRuns(
  suggestions: readonly Suggestion[],
): { group: Group; items: { suggestion: Suggestion; index: number }[] }[] {
  const runs: {
    group: Group;
    items: { suggestion: Suggestion; index: number }[];
  }[] = [];
  suggestions.forEach((suggestion, index) => {
    const last = runs.at(-1);
    if (last?.group === suggestion.group) {
      last.items.push({ suggestion, index });
    } else {
      runs.push({ group: suggestion.group, items: [{ suggestion, index }] });
    }
  });
  return runs;
}
