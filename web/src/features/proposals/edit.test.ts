import { describe, expect, it } from "vitest";
import type { VersionFile } from "@/api/proposals";
import { comparedFile } from "@/test/diff";
import {
  proposalDetail,
  proposalSummary,
  proposalVersion,
} from "@/test/proposals";
import {
  canEdit,
  canSendForReviewAgain,
  changed,
  editIntent,
  editNotice,
  editRequest,
  editSource,
  editableFile,
  inHumanReview,
  intentHint,
  intentProblem,
  shownIntentProblem,
  keptApprovalsNote,
  savedContent,
} from "./edit";

const MARK = { reason: "auth", set_by: "planner" };

function marked(reviewer: string | null, fields = {}) {
  return proposalDetail(
    proposalSummary("P-4", {
      current_version: 2,
      human_review: { ...MARK, reviewer },
      ...fields,
    }),
  );
}

function file(fields: Partial<VersionFile> = {}): VersionFile {
  return {
    path: "src/lib.rs",
    kind: "file",
    hash: "a".repeat(64),
    size: 4,
    binary: false,
    too_large: false,
    text: "a\nb\n",
    ...fields,
  };
}

describe("who edits", () => {
  it("is the human a human review names, or any human when it names none", () => {
    expect(inHumanReview(marked("tanim"), "tanim")).toBe(true);
    expect(inHumanReview(marked("tanim"), "ana")).toBe(false);
    expect(inHumanReview(marked(null), "ana")).toBe(true);
    expect(inHumanReview(marked(null), null)).toBe(false);
    expect(inHumanReview(proposalDetail(proposalSummary("P-4")), "ana")).toBe(
      false,
    );
    expect(inHumanReview(marked(null, { state: "closed" }), "ana")).toBe(false);
  });

  it("edits only the current version", () => {
    expect(canEdit(marked(null), "ana", { current: true })).toBe(true);
    expect(canEdit(marked(null), "ana", { current: false })).toBe(false);
    expect(canEdit(marked("tanim"), "ana", { current: true })).toBe(false);
  });

  it("opens a text file the version still has", () => {
    expect(editableFile(comparedFile("src/lib.rs", "modified"))).toBe(true);
    expect(editableFile(comparedFile("src/new.rs", "added"))).toBe(true);
    expect(editableFile(comparedFile("src/old.rs", "deleted"))).toBe(false);
    expect(
      editableFile({ ...comparedFile("logo.png", "modified"), binary: true }),
    ).toBe(false);
    expect(editableFile(null)).toBe(false);
  });
});

describe("the edit box content", () => {
  it("keeps a file's own line ending", () => {
    const unix = editSource(file());
    expect(unix).toEqual({ kind: "text", text: "a\nb\n", lineEnding: "\n" });
    const windows = editSource(file({ text: "a\r\nb\r\n" }));
    expect(windows).toEqual({
      kind: "text",
      text: "a\nb\n",
      lineEnding: "\r\n",
    });
    expect(savedContent("a\nc\n", "\r\n")).toBe("a\r\nc\r\n");
    expect(savedContent("a\r\nc\n", "\r\n")).toBe("a\r\nc\r\n");
    expect(savedContent("a\nc\n", "\n")).toBe("a\nc\n");
  });

  it("refuses what is not a text file", () => {
    for (const fields of [
      { kind: "symlink" as const },
      { kind: "directory" as const },
      { too_large: true, text: undefined },
      { binary: true, text: undefined },
      { text: undefined },
    ]) {
      expect(editSource(file(fields)).kind, JSON.stringify(fields)).toBe(
        "blocked",
      );
    }
    expect(editSource(file({ too_large: true }))).toEqual({
      kind: "blocked",
      reason: "The file is larger than 1 MiB, so it cannot be edited here.",
    });
  });

  it("saves the version on screen with the file's line ending, and needs a change", () => {
    const source = editSource(file({ text: "a\r\nb\r\n" }));
    if (source.kind !== "text") throw new Error("text expected");
    expect(changed(source, "a\nb\n")).toBe(false);
    expect(changed(source, "a\n")).toBe(true);
    expect(editRequest({ version: 2 }, "src/lib.rs", "a\n", source)).toEqual({
      version: 2,
      path: "src/lib.rs",
      content: "a\r\n",
    });
  });

  it("says what a save writes", () => {
    expect(editNotice(2)).toBe(
      "You are editing v2. Saving writes v3 under your name; v2 stays in the history. v3 keeps the agent approvals of v2, and you approve v3 again.",
    );
  });
});

describe("a human edit's approvals", () => {
  const edited = (fields = {}) =>
    proposalDetail(
      proposalSummary("P-4", {
        current_version: 2,
        human_review: { ...MARK, reviewer: null },
      }),
      {
        versions: [
          proposalVersion(1),
          proposalVersion(2, {
            created_by: "human:ana",
            human_edit: true,
            ...fields,
          }),
        ],
      },
    );

  it("sends the current human edit for review again once", () => {
    expect(canSendForReviewAgain(edited(), "ana")).toBe(true);
    expect(canSendForReviewAgain(edited({ review_again: true }), "ana")).toBe(
      false,
    );
    expect(canSendForReviewAgain(edited({ human_edit: false }), "ana")).toBe(
      false,
    );
    expect(canSendForReviewAgain(edited(), null)).toBe(false);
    const missing = { ...edited(), versions: [proposalVersion(1)] };
    expect(canSendForReviewAgain(missing, "ana")).toBe(false);
  });

  it("notes the kept approvals until the edit is sent for review again", () => {
    expect(keptApprovalsNote(edited(), 2)).toBe(
      "v2 is a human edit. It keeps the agent approvals of v1 until it is sent for review again.",
    );
    expect(keptApprovalsNote(edited({ review_again: true }), 2)).toBe(
      "v2 is a human edit that was sent for review again, so only new reviews count.",
    );
    expect(keptApprovalsNote(edited(), 1)).toBeNull();
    expect(keptApprovalsNote(edited(), 9)).toBeNull();
  });
});

describe("the intent of a human edit", () => {
  const source = {
    kind: "text" as const,
    text: "a\n",
    lineEnding: "\n" as const,
  };

  it("starts as the edited version's intent", () => {
    const p = proposalDetail(proposalSummary("P-4", { current_version: 2 }), {
      versions: [
        proposalVersion(1),
        proposalVersion(2, { intent: " Adds the table. " }),
      ],
    });
    expect(editIntent(p, 2)).toBe("Adds the table.");
    expect(editIntent(p, 1)).toBe("");
    expect(editIntent(p, 9)).toBe("");
  });

  it("sends the intent only when the human changed it", () => {
    const base = { version: 2, path: "a.txt", content: "b\n" };
    expect(
      editRequest({ version: 2 }, "a.txt", "b\n", source, " Kept. ", "Kept."),
    ).toEqual(base);
    expect(
      editRequest({ version: 2 }, "a.txt", "b\n", source, " ", ""),
    ).toEqual(base);
    expect(
      editRequest(
        { version: 2 },
        "a.txt",
        "b\n",
        source,
        " Returns 3. ",
        "Kept.",
      ),
    ).toEqual({ ...base, intent: "Returns 3." });
  });

  it("names what keeps Save off", () => {
    expect(intentProblem("Adds it.")).toBeNull();
    const empty =
      "Say what the new version does, why, what it does not change and its risks.";
    expect(intentProblem("  ")).toBe(empty);
    expect(intentProblem("")).toBe(empty);
    expect(intentProblem("é".repeat(2000))).toBeNull();
    expect(intentProblem("x".repeat(2001))).toBe(
      "The intent can hold at most 2000 characters; it has 2001.",
    );
  });

  it("asks for an intent on the edit of a version without one, without an error before the human writes", () => {
    expect(shownIntentProblem("", "")).toBeNull();
    expect(shownIntentProblem("  ", "")).toBeNull();
    expect(shownIntentProblem("x".repeat(2001), "")).toBe(
      "The intent can hold at most 2000 characters; it has 2001.",
    );
    expect(shownIntentProblem("", "Adds it.")).toBe(
      "Say what the new version does, why, what it does not change and its risks.",
    );
    expect(shownIntentProblem("Adds it.", "Adds it.")).toBeNull();
    expect(intentHint(2, "")).toBe(
      "What v3 does, why, what it does not change and its risks. v2 has no intent, so write one for v3.",
    );
    expect(intentHint(2, "Adds it.")).toBe(
      "What v3 does, why, what it does not change and its risks. It starts as the intent of v2; change what your edit changes.",
    );
  });
});
