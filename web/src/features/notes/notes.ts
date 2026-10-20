import type { NoteView } from "@/api/notes";
import { parseId } from "@/format/ids";
import { agentChatPath, projectPath, withQuery } from "@/routes/paths";

/** The area filter's value for the notes of the main agent and the review lead. No area name can be `-`, so it never names an area. */
export const NO_AREA = "-";

export interface NotesSearch {
  /** The agent that wrote the note. */
  agent?: string;
  /** An area name, or [`NO_AREA`]. */
  area?: string;
  /** The note to show first, such as `n-4`. */
  note?: string;
}

function text(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 && value.length <= 80
    ? value
    : undefined;
}

/** Keeps the search fields that hold a well-formed value; the URL is user input. */
export function parseNotesSearch(raw: Record<string, unknown>): NotesSearch {
  const search: NotesSearch = {};
  const agent = text(raw.agent);
  const area = text(raw.area);
  const note = text(raw.note);
  if (agent !== undefined) search.agent = agent;
  if (area !== undefined) search.area = area;
  if (note !== undefined && parseId(note)?.kind === "note") search.note = note;
  return search;
}

/** The notes path for a search, with only the set fields. */
export function notesHref(slug: string, search: NotesSearch): string {
  const params = new URLSearchParams();
  for (const key of ["agent", "area", "note"] as const) {
    const value = search[key];
    if (value !== undefined) params.set(key, value);
  }
  return withQuery(projectPath(slug, "notes"), params);
}

export interface NoteOptions {
  agents: string[];
  areas: string[];
  /** Whether some note belongs to no area. */
  unscoped: boolean;
}

/** The authors and areas the notes hold, each sorted, for the filter menus. */
export function noteOptions(notes: readonly NoteView[]): NoteOptions {
  const agents = new Set<string>();
  const areas = new Set<string>();
  let unscoped = false;
  for (const note of notes) {
    agents.add(note.agent);
    if (note.area === null) {
      unscoped = true;
    } else {
      areas.add(note.area);
    }
  }
  return {
    agents: [...agents].sort(),
    areas: [...areas].sort(),
    unscoped,
  };
}

/** The notes that both filters keep, in the daemon's order, newest first. */
export function filterNotes(
  notes: readonly NoteView[],
  search: NotesSearch,
): NoteView[] {
  return notes
    .filter((note) => search.agent === undefined || note.agent === search.agent)
    .filter((note) => {
      if (search.area === undefined) return true;
      if (search.area === NO_AREA) return note.area === null;
      return note.area === search.area;
    });
}

/** The area filter's button text. */
export function areaLabel(area: string | undefined): string {
  if (area === undefined) return "Area";
  return area === NO_AREA ? "Area: none" : `Area: ${area}`;
}

/** The ids of the newest `unread` notes: the daemon counts the notes past the human's read mark, and the list is newest first. */
export function newNoteIds(
  notes: readonly NoteView[],
  unread: number | null,
): Set<string> {
  return new Set(notes.slice(0, unread ?? 0).map((note) => note.id));
}

/** The note up to which the screen marks the notes as read, or null when nothing is left to mark: no human, no note or nothing unread. */
export function readMark(
  notes: readonly NoteView[],
  unread: number | null,
): string | null {
  const newest = notes[0];
  return unread !== null && unread > 0 && newest !== undefined
    ? newest.id
    : null;
}

/** The direct chat with the note's author, opened with the note named in the message to write. */
export function replyHref(slug: string, note: NoteView): string {
  return withQuery(agentChatPath(slug, note.agent), { note: note.id });
}
