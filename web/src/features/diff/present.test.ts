import { describe, expect, it } from "vitest";
import type { ComparedFile, ComparedState, Comparison } from "@/api/diff";
import { comparison as working_, state } from "@/test/diff";
import {
  NO_NEWLINE,
  binaryNote,
  compareOptions,
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
  tooLargeNote,
  warningMark,
  warningNote,
  workingNote,
} from "./present";

const workspace = state("workspace", "ws-7", {
  base: "r7",
  task: "T-12",
  worker: "frontend/ui",
});

function comparison(from: ComparedState, to: ComparedState): Comparison {
  return working_([], { from, to });
}

const working = comparison(state("revision", "r7"), workspace);
const againstOther = comparison(state("revision", "r6"), workspace);
const revisions = comparison(state("revision", "r7"), state("revision", "r8"));
const checkpoint = comparison(
  state("revision", "r7"),
  state("checkpoint", "ck-31", { task: "T-12" }),
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

  it("leads a checkpoint back to Repository", () => {
    expect(comparisonCrumbs("demo", checkpoint)[0]?.href).toBe(
      "/p/demo/repository",
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
    expect(changeNote(file({ change: "mode_changed" }))).toBe("mode changed");
    expect(changeNote(file({}))).toBeNull();
    expect(changeNote(file({ change: "renamed" }))).toBeNull();
  });
});

describe("row text", () => {
  it("names a line by kind, numbers and text", () => {
    const base = {
      type: "line" as const,
      key: "k",
      text: "x = 1",
      noNewline: false,
    };
    expect(lineLabel({ ...base, kind: "added", oldNo: null, newNo: 4 })).toBe(
      "Added line, new 4: x = 1",
    );
    expect(lineLabel({ ...base, kind: "removed", oldNo: 3, newNo: null })).toBe(
      "Removed line, old 3: x = 1",
    );
    expect(lineLabel({ ...base, kind: "context", oldNo: 2, newNo: 2 })).toBe(
      "Unchanged line, old 2, new 2: x = 1",
    );
    expect(
      lineLabel({
        ...base,
        kind: "added",
        oldNo: null,
        newNo: 5,
        noNewline: true,
      }),
    ).toBe(`Added line, new 5: x = 1, ${NO_NEWLINE}`);
  });

  it("counts hidden lines and remaining rows", () => {
    expect(hiddenLinesText(1)).toBe("Show 1 unchanged line");
    expect(hiddenLinesText(12)).toBe("Show 12 unchanged lines");
    expect(moreRowsText(500)).toBe("Show more · 500 more rows");
  });
});

describe("unreadable files", () => {
  it("marks an unstable file and says why it has no diff", () => {
    const unstable = { path: "a.log", reason: "unstable" as const };
    expect(warningMark(unstable)).toBe("unstable");
    expect(warningNote(unstable)).toContain("kept changing");
  });

  it("names every other reason", () => {
    const special = { path: "fifo", reason: "special" as const };
    expect(warningMark(special)).toBe("not a regular file");
    expect(warningNote(special)).toContain("could not read this file");
  });

  it("says a file over the size limit is compared by hash", () => {
    expect(tooLargeNote("modified")).toBe(
      "Modified file over 1 MiB, compared by hash.",
    );
  });
});

describe("compareOptions", () => {
  const recent = [
    { id: "r8", parent: "r7" },
    { id: "r7", parent: null },
  ];
  const checkpoints = [
    { id: "ck-30", label: null, automatic: true, created_at: 1 },
    { id: "ck-31", label: "before refactor", automatic: false, created_at: 2 },
  ];

  it("offers the base, main ahead of it and the checkpoints, newest first, as W06 lists them", () => {
    const options = compareOptions("demo", working, {
      base: "r7",
      main: { head: "r8", recent },
      checkpoints,
    });
    expect(options.map((o) => [o.text, o.current])).toEqual([
      ["base r7 (default)", true],
      ["main r8 · 1 revision ahead", false],
      ["checkpoint ck-31", false],
      ["checkpoint ck-30", false],
    ]);
    expect(options[1]?.href).toBe("/p/demo/diff?from=r8&to=ws-7");
  });

  it("leaves main out when it is the base or not read yet", () => {
    const same = compareOptions("demo", working, {
      base: "r8",
      main: { head: "r8", recent },
      checkpoints: [],
    });
    expect(same.map((o) => o.text)).toEqual(["base r8 (default)"]);
    const unread = compareOptions("demo", working, {
      base: "r7",
      main: null,
      checkpoints: [],
    });
    expect(unread).toHaveLength(1);
  });

  it("marks no option when the screen compares with a state the list does not hold", () => {
    const options = compareOptions("demo", againstOther, {
      base: "r7",
      main: { head: "r8", recent },
      checkpoints,
    });
    expect(options.some((o) => o.current)).toBe(false);
  });
});
