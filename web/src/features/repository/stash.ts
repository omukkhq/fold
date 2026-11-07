import type { StashEntry, StashSeen } from "@/api/stash";
import { formatAsOfMinute } from "@/format/time";

/** The most paths a stash row names; the rest are counted. */
export const LISTED_PATHS = 3;

/** One entry of the stash as the repository screen lists it. */
export interface StashRow {
  key: string;
  /** The entry's number, which stays with it while its place moves; a drop names it, so the daemon refuses the drop when another entry took the place. */
  number: number;
  /** `stash@{0}` for the newest. */
  name: string;
  /** Its place on the stack, which the drop route takes. */
  position: number;
  /** `2 files`. */
  files: string;
  /** The first paths of the entry, then `+3 more` when it has more. */
  paths: string[];
  /** The main revision the project folder held when the entry was made. */
  base: string;
  /** The event actor label of the human who stashed, such as `human:ana`. */
  actor: string;
  when: string;
  /** Its unstash conflict waits in the project folder, so it cannot be dropped. */
  conflicted: boolean;
}

export function filesText(count: number): string {
  return count === 1 ? "1 file" : `${String(count)} files`;
}

/** The place on the stack that `stash@{2}` names; the entry's index in the list when the name has another shape. */
export function positionOf(name: string, index: number): number {
  const match = /^stash@\{(\d+)\}$/.exec(name);
  return match?.[1] === undefined ? index : Number(match[1]);
}

/** The rows of the stash, newest first, with times as `14:12` or `Nov 7, 14:12`. */
export function stashRows(
  entries: readonly StashEntry[],
  now: number,
): StashRow[] {
  return entries.map((entry, index) => {
    const listed = entry.files.slice(0, LISTED_PATHS);
    const more = entry.files.length - listed.length;
    return {
      key: String(entry.number),
      number: entry.number,
      name: entry.stash,
      position: positionOf(entry.stash, index),
      files: filesText(entry.files.length),
      paths: more > 0 ? [...listed, `+${String(more)} more`] : listed,
      base: entry.base,
      actor: entry.actor,
      when: formatAsOfMinute(now, entry.created_at),
      conflicted: entry.conflicted.length > 0,
    };
  });
}

/** Whether the stash as read just now still holds the confirmed row at its place, so a drop removes what the person saw. */
export function rowStillThere(
  entries: readonly StashEntry[],
  row: StashRow,
): boolean {
  const entry = entries.find(
    (candidate, index) => positionOf(candidate.stash, index) === row.position,
  );
  return entry?.number === row.number;
}

/** What Clear all confirmed: how many entries, and the number of the newest. */
export function clearSeen(rows: readonly StashRow[]): StashSeen {
  return { entries: rows.length, newest: rows[0]?.number ?? null };
}

/** Whether the stash as read just now is the one Clear all confirmed. */
export function stashStillSeen(
  entries: readonly StashEntry[],
  seen: StashSeen,
): boolean {
  return (
    entries.length === seen.entries &&
    (entries[0]?.number ?? null) === seen.newest
  );
}

/** Clear all drops every entry, which the daemon refuses as a whole while one entry's unstash conflict waits. */
export function canClear(rows: readonly StashRow[]): boolean {
  return rows.length > 0 && rows.every((row) => !row.conflicted);
}

export function dropTitle(row: StashRow): string {
  return `Drop ${row.name}?`;
}

export function dropDescription(row: StashRow): string {
  return `The changes of ${row.files} in ${row.name} are gone for good; fld unstash cannot bring them back. No file in the project folder changes.`;
}

export function clearTitle(count: number): string {
  return count === 1
    ? "Drop the stash entry?"
    : `Drop all ${String(count)} stash entries?`;
}

export function clearDescription(count: number): string {
  return count === 1
    ? "The changes of the entry are gone for good; fld unstash cannot bring them back. No file in the project folder changes."
    : "The changes of every entry are gone for good; fld unstash cannot bring them back. No file in the project folder changes.";
}

/** Why a row has no drop action. */
export const CONFLICTED_NOTE =
  "Its unstash conflict waits in the project folder. Fix the files and run `fld record`, or reset the folder.";
