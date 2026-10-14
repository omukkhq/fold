import { describe, expect, it } from "vitest";
import type { ComparedFile, ComparedState, Comparison } from "@/api/diff";
import {
  binaryNote,
  changeBadge,
  changeNote,
  comparisonCrumbs,
  comparisonNote,
  comparisonTitle,
  hiddenLinesText,
  isWorkingChanges,
  lineCounts,
  lineLabel,
  moreRowsText,
  stateLabel,
  workingNote,
} from "./present";

function state(
  kind: ComparedState["kind"],
  id: string,
  extra: Partial<ComparedState> = {},
): ComparedState {
  return { kind, id, base: null, task: null, worker: null, ...extra };
}

const workspace = state("workspace", "ws-7", {
  base: "r7",
  task: "T-12",
  worker: "frontend/ui",
});

function comparison(from: ComparedState, to: ComparedState): Comparison {
  return { from, to, read_at: null, files: [] };
}

const working = comparison(state("revision", "r7"), workspace);
const againstOther = comparison(state("revision", "r6"), workspace);
const revisions = comparison(state("revision", "r7"), state("revision", "r8"));
const proposal = comparison(
  state("revision", "r7"),
  state("proposal_version", "P-3 v2"),
);

function file(extra: Partial<ComparedFile>): ComparedFile {
  return {
    path: "a.ts",
    previous_path: null,
    change: "modified",
    added_lines: 0,
    removed_lines: 0,
    binary: false,
    old_hash: "h1",
    new_hash: "h2",
    ...extra,
  };
}

describe("working changes", () => {
  it("is a workspace's working files against its base", () => {
    expect(isWorkingChanges(working)).toBe(true);
  });

  it("is not a workspace against another revision, nor two revisions", () => {
    expect(isWorkingChanges(againstOther)).toBe(false);
    expect(isWorkingChanges(revisions)).toBe(false);
    expect(
      isWorkingChanges(comparison(workspace, state("revision", "r7"))),
    ).toBe(false);
  });

  it("labels the header Working changes", () => {
    expect(comparisonTitle(working)).toBe("Working changes in ws-7");
  });

  it("says the files are not history and names the task", () => {
    expect(workingNote(working)).toContain("They are not history");
    expect(workingNote(working)).toContain(
      "until T-12's proposal is integrated",
    );
    const noTask = comparison(state("revision", "r7"), {
      ...workspace,
      task: null,
    });
    expect(workingNote(noTask)).toContain("until it is integrated");
  });
});

describe("comparisonTitle", () => {
  it("names a workspace against another revision", () => {
    expect(comparisonTitle(againstOther)).toBe(
      "Working files in ws-7 against r6",
    );
  });

  it("names two states", () => {
    expect(comparisonTitle(revisions)).toBe("Changes from r7 to r8");
  });
});

describe("comparisonCrumbs", () => {
  it("leads a workspace back to Activity", () => {
    expect(comparisonCrumbs("demo", working)).toEqual([
      { label: "Activity", href: "/p/demo/activity", mono: false },
      { label: "ws-7", href: null, mono: true },
    ]);
  });

  it("leads a proposal version back to Proposals", () => {
    expect(comparisonCrumbs("demo", proposal)[0]?.href).toBe(
      "/p/demo/proposals",
    );
  });

  it("leads revisions back to Repository", () => {
    expect(comparisonCrumbs("demo", revisions)).toEqual([
      { label: "Repository", href: "/p/demo/repository", mono: false },
      { label: "r7 → r8", href: null, mono: true },
    ]);
  });
});

describe("stateLabel", () => {
  it("calls the workspace side Working files and its base side base r7", () => {
    expect(stateLabel(working.to, working)).toBe("Working files");
    expect(stateLabel(working.from, working)).toBe("base r7");
  });

  it("uses the plain id elsewhere", () => {
    expect(stateLabel(againstOther.from, againstOther)).toBe("r6");
    expect(stateLabel(revisions.to, revisions)).toBe("r8");
  });
});

describe("comparisonNote", () => {
  const files = [
    file({ added_lines: 42, removed_lines: 8 }),
    file({ added_lines: 31 }),
  ];

  it("says working changes are not in main and gives task, worker and counts", () => {
    expect(comparisonNote(working, files)).toBe(
      "Not in main. T-12 · frontend/ui · 2 files · +73 −8",
    );
  });

  it("leaves out a missing task or worker", () => {
    const bare = comparison(
      state("revision", "r7"),
      state("workspace", "ws-7", { base: "r7" }),
    );
    expect(comparisonNote(bare, [file({})])).toBe(
      "Not in main. 1 file · +0 −0",
    );
  });

  it("gives the counts alone for other states", () => {
    expect(comparisonNote(revisions, files)).toBe("2 files · +73 −8");
  });
});

describe("files", () => {
  it("gives each change kind a letter, a name and its colours", () => {
    expect(changeBadge("added")).toMatchObject({ letter: "A", label: "Added" });
    expect(changeBadge("modified").letter).toBe("M");
    expect(changeBadge("deleted").classes).toContain("text-danger");
    expect(changeBadge("renamed").letter).toBe("R");
  });

  it("names the change kind of a binary file", () => {
    expect(binaryNote("modified")).toBe(
      "Modified binary file, compared by hash.",
    );
    expect(binaryNote("added")).toBe("Added binary file, compared by hash.");
    expect(binaryNote("deleted")).toBe(
      "Deleted binary file, compared by hash.",
    );
  });

  it("writes counts with U+2212 and leaves out zero", () => {
    expect(lineCounts({ added_lines: 3, removed_lines: 2 })).toEqual({
      added: "+3",
      removed: "−2",
    });
    expect(lineCounts({ added_lines: 0, removed_lines: 0 })).toEqual({
      added: null,
      removed: null,
    });
  });

  it("notes a rename, a new file and a deleted file", () => {
    expect(
      changeNote(file({ change: "renamed", previous_path: "old.ts" })),
    ).toBe("renamed from old.ts");
    expect(changeNote(file({ change: "added" }))).toBe("new file");
    expect(changeNote(file({ change: "deleted" }))).toBe("deleted");
    expect(changeNote(file({}))).toBeNull();
    expect(changeNote(file({ change: "renamed" }))).toBeNull();
  });
});

describe("row text", () => {
  it("names a line by kind, numbers and text", () => {
    const base = { type: "line" as const, key: "k", text: "x = 1" };
    expect(lineLabel({ ...base, kind: "add", oldNo: null, newNo: 4 })).toBe(
      "Added line, new 4: x = 1",
    );
    expect(lineLabel({ ...base, kind: "remove", oldNo: 3, newNo: null })).toBe(
      "Removed line, old 3: x = 1",
    );
    expect(lineLabel({ ...base, kind: "context", oldNo: 2, newNo: 2 })).toBe(
      "Unchanged line, old 2, new 2: x = 1",
    );
  });

  it("counts hidden lines and remaining rows", () => {
    expect(hiddenLinesText(1)).toBe("Show 1 unchanged line");
    expect(hiddenLinesText(12)).toBe("Show 12 unchanged lines");
    expect(moreRowsText(500)).toBe("Show more · 500 more rows");
  });
});
