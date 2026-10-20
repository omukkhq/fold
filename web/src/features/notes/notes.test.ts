import { describe, expect, it } from "vitest";
import type { NoteView } from "@/api/notes";
import {
  NO_AREA,
  areaLabel,
  filterNotes,
  newNoteIds,
  noteOptions,
  notesHref,
  parseNotesSearch,
  readMark,
  replyHref,
} from "./notes";

const note = (
  id: string,
  agent: string,
  area: string | null,
  author = agent,
): NoteView => ({
  id,
  author,
  agent,
  area,
  title: `Title of ${id}`,
  decision: "d",
  reason: "r",
  links: [],
  written_at: 0,
});

const NOTES = [
  note("n-4", "planner", null),
  note("n-3", "web-lead", "web"),
  note("n-2", "review-lead", null),
  note("n-1", "api-lead", "api", "api-lead/helper"),
];

describe("parseNotesSearch", () => {
  it("keeps well-formed values and drops the rest", () => {
    expect(
      parseNotesSearch({ agent: "web-lead", area: "web", note: "n-3" }),
    ).toEqual({ agent: "web-lead", area: "web", note: "n-3" });
    expect(
      parseNotesSearch({ agent: "", area: 3, note: "T-3", other: "x" }),
    ).toEqual({});
    expect(parseNotesSearch({ agent: "a".repeat(81) })).toEqual({});
  });
});

describe("notesHref", () => {
  it("writes only the set fields", () => {
    expect(notesHref("demo", {})).toBe("/p/demo/notes");
    expect(notesHref("demo", { area: NO_AREA, note: "n-2" })).toBe(
      "/p/demo/notes?area=-&note=n-2",
    );
    expect(notesHref("demo", { agent: "web-lead" })).toBe(
      "/p/demo/notes?agent=web-lead",
    );
  });
});

describe("noteOptions", () => {
  it("lists the sorted authors and areas and whether a note has no area", () => {
    expect(noteOptions(NOTES)).toEqual({
      agents: ["api-lead", "planner", "review-lead", "web-lead"],
      areas: ["api", "web"],
      unscoped: true,
    });
    expect(noteOptions([note("n-1", "web-lead", "web")]).unscoped).toBe(false);
  });
});

describe("filterNotes", () => {
  const ids = (notes: NoteView[]) => notes.map((n) => n.id);

  it("keeps every note without filters", () => {
    expect(ids(filterNotes(NOTES, {}))).toEqual(["n-4", "n-3", "n-2", "n-1"]);
  });

  it("filters by author, by area and by no area", () => {
    expect(ids(filterNotes(NOTES, { agent: "api-lead" }))).toEqual(["n-1"]);
    expect(ids(filterNotes(NOTES, { area: "web" }))).toEqual(["n-3"]);
    expect(ids(filterNotes(NOTES, { area: NO_AREA }))).toEqual(["n-4", "n-2"]);
    expect(ids(filterNotes(NOTES, { agent: "planner", area: "web" }))).toEqual(
      [],
    );
  });
});

describe("areaLabel", () => {
  it("names the chosen area", () => {
    expect(areaLabel(undefined)).toBe("Area");
    expect(areaLabel(NO_AREA)).toBe("Area: none");
    expect(areaLabel("web")).toBe("Area: web");
  });
});

describe("newNoteIds", () => {
  it("takes the newest unread notes", () => {
    expect([...newNoteIds(NOTES, 2)]).toEqual(["n-4", "n-3"]);
    expect(newNoteIds(NOTES, 0).size).toBe(0);
    expect(newNoteIds(NOTES, null).size).toBe(0);
  });
});

describe("readMark", () => {
  it("marks up to the newest note only while some note is unread", () => {
    expect(readMark(NOTES, 1)).toBe("n-4");
    expect(readMark(NOTES, 0)).toBeNull();
    expect(readMark(NOTES, null)).toBeNull();
    expect(readMark([], 3)).toBeNull();
  });
});

describe("replyHref", () => {
  it("opens the chat with the author's agent and names the note", () => {
    expect(replyHref("demo", NOTES[3] ?? note("n-9", "x", null))).toBe(
      "/p/demo/agents/api-lead/chat?note=n-1",
    );
  });
});
