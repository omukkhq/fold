import { describe, expect, it } from "vitest";
import type { StashEntry } from "@/api/stash";
import {
  canClear,
  clearDescription,
  clearSeen,
  clearTitle,
  dropDescription,
  dropTitle,
  positionOf,
  rowStillThere,
  stashRows,
  stashStillSeen,
} from "./stash";

const NOW = new Date(2026, 10, 7, 15, 0).getTime();

function entry(overrides: Partial<StashEntry> = {}): StashEntry {
  return {
    stash: "stash@{0}",
    number: 1,
    created_at: new Date(2026, 10, 7, 14, 12).getTime(),
    base: "r8",
    actor: "human:ana",
    files: ["plan.md"],
    conflicted: [],
    ...overrides,
  };
}

describe("the stash rows", () => {
  it("name each entry, its files, its base and its time, newest first", () => {
    const rows = stashRows(
      [
        entry(),
        entry({
          stash: "stash@{1}",
          number: 2,
          created_at: new Date(2026, 10, 6, 9, 5).getTime(),
          files: ["a", "b", "c", "d", "e"],
        }),
      ],
      NOW,
    );
    expect(
      rows.map((row) => [row.name, row.position, row.files, row.when]),
    ).toEqual([
      ["stash@{0}", 0, "1 file", "14:12"],
      ["stash@{1}", 1, "5 files", "Nov 6, 09:05"],
    ]);
    expect(rows[0]?.paths).toEqual(["plan.md"]);
    expect(rows[1]?.paths).toEqual(["a", "b", "c", "+2 more"]);
    expect(rows[0]?.base).toBe("r8");
    expect(rows[0]?.actor).toBe("human:ana");
    expect(rows[0]?.key).not.toBe(rows[1]?.key);
  });

  it("read the place from the name, and fall back to the index", () => {
    expect(positionOf("stash@{12}", 0)).toBe(12);
    expect(positionOf("odd", 3)).toBe(3);
  });

  it("allow Clear all only while no entry's unstash conflict waits", () => {
    expect(canClear([])).toBe(false);
    const clean = stashRows([entry(), entry({ stash: "stash@{1}" })], NOW);
    expect(canClear(clean)).toBe(true);
    const waiting = stashRows([entry({ conflicted: ["plan.md"] })], NOW);
    expect(waiting[0]?.conflicted).toBe(true);
    expect(canClear(waiting)).toBe(false);
  });

  it("ask before a drop, and say the changes are gone for good", () => {
    const [row] = stashRows(
      [entry({ stash: "stash@{2}", files: ["a", "b"] })],
      NOW,
    );
    if (!row) throw new Error("a row");
    expect(dropTitle(row)).toBe("Drop stash@{2}?");
    expect(dropDescription(row)).toBe(
      "The changes of 2 files in stash@{2} are gone for good; fld unstash cannot bring them back. No file in the project folder changes.",
    );
    expect(clearTitle(1)).toBe("Drop the stash entry?");
    expect(clearTitle(3)).toBe("Drop all 3 stash entries?");
    expect(clearDescription(1)).toContain("The changes of the entry");
    expect(clearDescription(3)).toContain("every entry");
  });

  it("drop only the entry the person confirmed, which a newer entry pushed to another place no longer is", () => {
    const junk = entry({ number: 7, files: ["junk"] });
    const [row] = stashRows([junk], NOW);
    if (!row) throw new Error("a row");
    expect(row.number).toBe(7);
    expect(rowStillThere([junk], row)).toBe(true);
    const safety = entry({ number: 8, files: ["plan.md"] });
    expect(rowStillThere([safety, { ...junk, stash: "stash@{1}" }], row)).toBe(
      false,
    );
    expect(rowStillThere([], row)).toBe(false);
  });

  it("clear only the stash the person confirmed: the same count and the same newest entry", () => {
    const listed = [
      entry({ number: 4 }),
      entry({ stash: "stash@{1}", number: 3 }),
    ];
    const seen = clearSeen(stashRows(listed, NOW));
    expect(seen).toEqual({ entries: 2, newest: 4 });
    expect(stashStillSeen(listed, seen)).toBe(true);
    expect(stashStillSeen(listed.slice(0, 1), seen)).toBe(false);
    expect(
      stashStillSeen(
        [entry({ number: 5 }), entry({ stash: "stash@{1}", number: 3 })],
        seen,
      ),
    ).toBe(false);
    expect(clearSeen([])).toEqual({ entries: 0, newest: null });
    expect(stashStillSeen([], clearSeen([]))).toBe(true);
    expect(stashStillSeen([entry()], clearSeen([]))).toBe(false);
  });
});
