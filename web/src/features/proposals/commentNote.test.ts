import { describe, expect, it } from "vitest";
import {
  proposalDetail,
  proposalSummary,
  proposalVersion,
} from "@/test/proposals";
import { commentNote } from "./commentNote";
import { OLD_SIDE_NOTE, resolveView } from "./view";

function proposal(state: "open" | "integrated" = "open") {
  return proposalDetail(proposalSummary("P-1", { current_version: 2, state }), {
    versions: [proposalVersion(1), proposalVersion(2)],
  });
}

describe("commentNote", () => {
  it("opens the comment box for a named human on the current version, on either side of a full change", () => {
    const p = proposal();
    const view = resolveView(p, {});
    const comment = { kind: "comment", human: "rafeed" };
    expect(commentNote(p, "rafeed", view, "new", "a.toml")).toEqual(comment);
    expect(commentNote(p, "rafeed", view, "old", "a.toml")).toEqual(comment);
  });

  it("asks for a name first", () => {
    const p = proposal();
    expect(commentNote(p, null, resolveView(p, {}), "new", "a.toml")).toEqual({
      kind: "note",
      text: "Tell fold your name first, then comment.",
      link: null,
    });
  });

  it("names the state of a proposal that is not open, before a missing name", () => {
    const p = proposal("integrated");
    for (const human of ["rafeed", null]) {
      expect(
        commentNote(p, human, resolveView(p, {}), "new", "a.toml"),
      ).toEqual({
        kind: "note",
        text: "P-1 is integrated, so it takes no new comments.",
        link: null,
      });
    }
  });

  it("sends a selection on an older version to the current one, before a missing name", () => {
    const p = proposal();
    for (const human of ["rafeed", null]) {
      expect(
        commentNote(p, human, resolveView(p, { version: 1 }), "new", "a.toml"),
      ).toEqual({
        kind: "note",
        text: "This is v1. Comments go on the current version, v2.",
        link: { label: "Open v2", search: { path: "a.toml" } },
      });
    }
  });

  it("allows the new side only on a change since an earlier version, and says so before a missing name", () => {
    const p = proposal();
    const view = resolveView(p, { since: 1 });
    expect(commentNote(p, "rafeed", view, "new", "a.toml")).toEqual({
      kind: "comment",
      human: "rafeed",
    });
    const oldSide = { kind: "note", text: OLD_SIDE_NOTE, link: null };
    expect(commentNote(p, "rafeed", view, "old", "a.toml")).toEqual(oldSide);
    expect(commentNote(p, null, view, "old", "a.toml")).toEqual(oldSide);
  });
});
