import { describe, expect, it } from "vitest";
import { liveWorkspace, taskView } from "@/test/payloads";
import {
  PROPOSAL_NOTE,
  linkGaps,
  nothingText,
  peopleGaps,
  workGaps,
} from "./sideGroups";

const checkpoint = {
  id: "ck-3",
  label: null,
  automatic: true,
  created_at: 5,
};

describe("nothingText", () => {
  it("joins the empty items into one line and adds the note", () => {
    expect(nothingText({ items: [], note: "Never shown." })).toBeNull();
    expect(nothingText({ items: ["no blocker"], note: null })).toBe(
      "no blocker.",
    );
    expect(
      nothingText({ items: ["no changesets", "no proposal"], note: "Soon." }),
    ).toBe("no changesets, no proposal. Soon.");
  });
});

describe("peopleGaps", () => {
  it("names a claim nobody holds", () => {
    expect(peopleGaps(taskView("T-1", { claims: [] })).items).toEqual([
      "nobody holds the claim",
    ]);
    expect(
      peopleGaps(
        taskView("T-1", {
          claims: [
            {
              holder: "web",
              kind: "implementation",
              claimed_at: 0,
              renewed_at: 0,
              stale: false,
            },
          ],
        }),
      ).items,
    ).toEqual([]);
  });
});

describe("workGaps", () => {
  it("names every empty item of a task that has not started", () => {
    expect(workGaps(taskView("T-1", { workspace: null }), null)).toEqual({
      items: ["no workspace", "no checkpoints", "no changesets", "no proposal"],
      note: PROPOSAL_NOTE,
    });
  });

  it("counts the checkpoints of the live frame, and drops the note once a proposal exists", () => {
    const task = taskView("T-1", {
      workspace: "ws-1",
      changesets: [{ id: "C-1", message: "Add", created_at: 1 }],
      proposal: {
        id: "P-1",
        title: "Add",
        review_claim: null,
        files: 1,
        state: "open",
        version: 1,
        checks_failed: false,
        reviews: [],
      },
    });
    expect(workGaps(task, null)).toEqual({
      items: ["no checkpoints"],
      note: null,
    });
    expect(
      workGaps(
        task,
        liveWorkspace("ws-1", "T-1", { checkpoints: [checkpoint] }),
      ),
    ).toEqual({ items: [], note: null });
  });
});

describe("linkGaps", () => {
  it("names the blocker and each side of the dependencies that is empty", () => {
    expect(linkGaps(taskView("T-1", { dependencies: [] })).items).toEqual([
      "no blocker",
      "no prerequisites",
      "no task waits for this one",
    ]);
    expect(
      linkGaps(
        taskView("T-1", {
          blocker: { reason: "Wait", set_by: "tanim", set_at: 0 },
          dependencies: [
            {
              task: "T-2",
              title: "Two",
              status: "todo",
              direction: "needs",
              origin: "direct",
            },
          ],
        }),
      ).items,
    ).toEqual(["no task waits for this one"]);
  });
});
