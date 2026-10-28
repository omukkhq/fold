import { describe, expect, it } from "vitest";
import type { BoardCard } from "@/api/board";
import type { LiveWorkspace } from "@/live/envelope";
import { activityLine, liveFilesText } from "./activityLine";

const NOW = Date.UTC(2026, 9, 15, 12, 0, 0);
const SECOND = 1_000;
const MINUTE = 60 * SECOND;

function card(
  activity: BoardCard["activity"],
  attention: BoardCard["attention"] = [],
  blocked = false,
): Pick<
  BoardCard,
  "activity" | "attention" | "blocked" | "proposal" | "edits_after_version"
> {
  return {
    activity,
    attention,
    blocked,
    proposal: null,
    edits_after_version: null,
  };
}

function live(at: number, files: number): LiveWorkspace {
  return {
    workspace: "ws-1",
    task: "T-1",
    worker: "frontend/ui",
    base: "r1",
    files: Array.from({ length: files }, (_, i) => ({
      path: `f${String(i)}.ts`,
      previous_path: null,
      change: "modified" as const,
      added_lines: 1,
      removed_lines: 0,
      binary: false,
    })),
    summary: "",
    last_activity_at: at,
    checkpoints: [],
    proposal: null,
    stalled: false,
    edits_after_version: null,
    check_run: null,
  };
}

describe("activityLine", () => {
  it("is null with no activity and no live workspace", () => {
    expect(activityLine(card(null), undefined, NOW)).toBeNull();
  });

  it("names the file being edited and the time since the last change", () => {
    expect(
      activityLine(
        card({ path: "src/routes/login.tsx", at: NOW - 12 * SECOND }),
        undefined,
        NOW,
      ),
    ).toEqual({
      text: "Editing src/routes/login.tsx",
      since: "12 s ago",
      stalled: false,
      files: null,
    });
  });

  it("says Editing alone when the daemon names no file", () => {
    expect(
      activityLine(card({ path: null, at: NOW }), undefined, NOW)?.text,
    ).toBe("Editing");
  });

  it("says no edits for n min when the attention list has stalled", () => {
    const line = activityLine(
      card({ path: "a.ts", at: NOW - 6 * MINUTE }, [
        { kind: "stalled", closable: false },
      ]),
      undefined,
      NOW,
    );
    expect(line).toMatchObject({
      text: "No edits for 6 min",
      since: "6 min ago",
      stalled: true,
    });
  });

  it("adds blocked to a stalled line when the task waits on a prerequisite", () => {
    expect(
      activityLine(
        card(
          { path: "a.ts", at: NOW - 6 * MINUTE },
          [{ kind: "stalled", closable: false }],
          true,
        ),
        undefined,
        NOW,
      )?.text,
    ).toBe("No edits for 6 min · blocked");
    expect(
      activityLine(card({ path: "a.ts", at: NOW }, [], true), undefined, NOW)
        ?.text,
    ).toBe("Editing a.ts");
  });

  it("does not call a quiet card stalled without the attention item", () => {
    const line = activityLine(
      card({ path: "a.ts", at: NOW - 30 * MINUTE }),
      undefined,
      NOW,
    );
    expect(line?.stalled).toBe(false);
    expect(line?.text).toBe("Editing a.ts");
  });

  it("moves the time forward with a later live frame and counts its files", () => {
    const line = activityLine(
      card({ path: "a.ts", at: NOW - 5 * MINUTE }),
      live(NOW - 3 * SECOND, 4),
      NOW,
    );
    expect(line).toMatchObject({
      text: "Editing a.ts",
      since: "3 s ago",
      files: 4,
    });
  });

  it("keeps the payload time when the live frame is older", () => {
    const line = activityLine(
      card({ path: "a.ts", at: NOW - 2 * SECOND }),
      live(NOW - MINUTE, 1),
      NOW,
    );
    expect(line?.since).toBe("2 s ago");
  });

  it("names the one changed file of a live frame when the payload has no activity", () => {
    expect(activityLine(card(null), live(NOW - 3 * SECOND, 1), NOW)?.text).toBe(
      "Editing f0.ts",
    );
    expect(activityLine(card(null), live(NOW, 3), NOW)?.text).toBe("Editing");
  });

  it("says the holder sent the version and what it waits for while the files are that version", () => {
    const sent = {
      ...card({ path: "a.ts", at: NOW - 6 * MINUTE }, [
        { kind: "stalled", closable: false },
        { kind: "human_review", closable: false },
      ]),
      proposal: {
        id: "P-1",
        state: "open" as const,
        version: 2,
        checks_failed: false,
        revision: null,
        review_plan: null,
      },
      edits_after_version: false,
    };
    expect(activityLine(sent, undefined, NOW)).toEqual({
      text: "Sent P-1 v2 · waits for human review",
      since: "6 min ago",
      stalled: false,
      files: null,
    });
    const frame = { ...live(NOW - SECOND, 1), proposal: "P-1" };
    expect(
      activityLine(
        { ...sent, attention: [] },
        { ...frame, edits_after_version: true },
        NOW,
      )?.text,
    ).toBe("Editing a.ts");
    expect(
      activityLine(
        { ...sent, edits_after_version: true },
        { ...frame, edits_after_version: false },
        NOW,
      )?.text,
    ).toBe("Sent P-1 v2 · waits for human review");
  });

  it("says the stall while a sent version is back with its author", () => {
    const back = {
      ...card({ path: "a.ts", at: NOW - 6 * MINUTE }, [
        { kind: "stalled", closable: false },
      ]),
      proposal: {
        id: "P-1",
        state: "changes_requested" as const,
        version: 1,
        checks_failed: false,
        revision: null,
        review_plan: null,
      },
      edits_after_version: false,
    };
    expect(activityLine(back, undefined, NOW)).toEqual({
      text: "No edits for 6 min",
      since: "6 min ago",
      stalled: true,
      files: null,
    });
    expect(activityLine({ ...back, attention: [] }, undefined, NOW)?.text).toBe(
      "Sent P-1 v1 · changes requested",
    );
  });

  it("works from a card whose payload has no activity", () => {
    const bare = {
      activity: null,
      attention: [],
      blocked: false,
      proposal: null,
      edits_after_version: null,
    };
    expect(activityLine(bare, undefined, NOW)).toBeNull();
    expect(activityLine(bare, live(NOW, 0), NOW)?.text).toBe("Editing");
  });

  it("shows a live workspace that the payload has no activity for", () => {
    expect(activityLine(card(null), live(NOW - MINUTE, 2), NOW)).toEqual({
      text: "Editing",
      since: "1 min ago",
      stalled: false,
      files: 2,
    });
  });
});

describe("liveFilesText", () => {
  const base = { text: "Editing", since: "12 s ago", stalled: false };

  it("counts the changed files", () => {
    expect(liveFilesText({ ...base, files: 4 })).toBe("4 files");
    expect(liveFilesText({ ...base, files: 1 })).toBe("1 file");
  });

  it("is null without a line, a count, any file, or while stalled", () => {
    expect(liveFilesText(null)).toBeNull();
    expect(liveFilesText({ ...base, files: null })).toBeNull();
    expect(liveFilesText({ ...base, files: 0 })).toBeNull();
    expect(liveFilesText({ ...base, files: 3, stalled: true })).toBeNull();
  });
});
