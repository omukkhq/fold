import type { ComparedFile } from "@/api/diff";
import type { Intent, IntentPart } from "@/api/proposals";
import { matchesFilter } from "./folderTree";
import { partOf } from "./intent";

/** The name of the group of changed files that no part of the intent names. */
export const OUTSIDE_INTENT = "Not in the intent";

/** The name of the group, in a change since an earlier version, of the files the shown version changed back to the base. */
export const BACK_TO_BASE = "Changed back to the base";

/** The changed files of one part of the intent, in the order the part lists them, or the files no part names. */
export interface PartGroup {
  name: string;
  /** What the part does; null for the files outside the intent. */
  does: string | null;
  files: ComparedFile[];
  /** The files no part of the intent names; the daemon refuses such a version, so only a version it could not check shows them. */
  outside: boolean;
}

/**
 * The files of a comparison grouped by the part of `intent` each belongs to, parts in the intent's order and files in each part's order, then the files no part names. A part without a file in the comparison, as in a change since an earlier version, is left out. null without an intent, so the screen shows its folder tree.
 * The parts cover every file the version changes against its base, so in a change `since` an earlier version a file no part names is one the version changed back to the base: it goes into a group of its own that needs no attention.
 */
export function partGroups(
  intent: Intent | null,
  files: readonly ComparedFile[],
  since = false,
): PartGroup[] | null {
  if (intent === null) return null;
  const byPath = new Map(files.map((file) => [file.path, file]));
  const named = new Set<string>();
  const groups: PartGroup[] = [];
  for (const part of intent.parts) {
    const own = part.files.flatMap((path) => {
      const file = byPath.get(path);
      if (file === undefined || named.has(path)) return [];
      named.add(path);
      return [file];
    });
    if (own.length > 0) {
      groups.push({
        name: part.name,
        does: part.does,
        files: own,
        outside: false,
      });
    }
  }
  const outside = files.filter((file) => !named.has(file.path));
  if (outside.length > 0) {
    groups.push({
      name: since ? BACK_TO_BASE : OUTSIDE_INTENT,
      does: null,
      files: outside,
      outside: !since,
    });
  }
  return groups;
}

/** What the note above a file's lines says: the part it belongs to, that no part names it, or that the shown version changed it back to the base. */
export type FileNote =
  | { kind: "part"; part: IntentPart }
  | { kind: "outside" }
  | { kind: "back" };

export function fileNote(
  intent: Intent,
  path: string,
  since: boolean,
): FileNote {
  const part = partOf(intent, path);
  if (part !== null) return { kind: "part", part };
  return since ? { kind: "back" } : { kind: "outside" };
}

/** The groups with only the files that match the filter; a group without a match is left out. */
export function filterGroups(
  groups: readonly PartGroup[],
  filter: string,
): PartGroup[] {
  return groups.flatMap((group) => {
    const files = group.files.filter((file) =>
      matchesFilter(file.path, filter),
    );
    return files.length === 0 ? [] : [{ ...group, files }];
  });
}

/** The files in group order, the order the tree lists them. */
export function groupFiles(groups: readonly PartGroup[]): ComparedFile[] {
  return groups.flatMap((group) => group.files);
}
