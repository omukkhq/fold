import { describe, expect, it } from "vitest";
import { boardCard } from "@/test/payloads";
import { PREVIEW_CHARS, PREVIEW_LINES, cardDetails, proseRow } from "./details";

describe("cardDetails", () => {
  it("heads the panel with the id and the column", () => {
    const details = cardDetails(
      boardCard("T-4", { status: "in_progress" }),
      null,
      0,
    );
    expect(details.head).toBe("T-4 · In progress");
    expect(details.rows).toEqual([]);
    expect(details.why).toBeNull();
    expect(details.notes).toEqual([]);
  });

  it("lists every fact the card leaves out, in order", () => {
    const card = boardCard("T-7", {
      status: "review",
      worker: "api-audit",
      kind: "no_proposal",
      report: "Drew the icons.",
      blocker: { reason: "Wait for the logo", set_by: "tanim", set_at: 0 },
      blocked_by: ["T-2", "T-9"],
      blocks: 1,
      messages: 2,
      human_review: { reviewer: "dev", reason: "Auth", set_by: "planner" },
      checklist: {
        steps_done: 1,
        steps_total: 2,
        linked_done: 0,
        linked_total: 1,
      },
      labels: [{ name: "backend", color: "sage" }],
      proposal: {
        id: "P-4",
        state: "open",
        version: 1,
        checks_failed: false,
        revision: null,
        review_plan: {
          plan: "rp-2",
          awaited: ["T-9"],
          deadline_at: 0,
          wait_ended: false,
          proposals: ["P-4"],
        },
        review_claim: null,
      },
      attention: [
        { kind: "stalled", closable: false },
        { kind: "stale_claim", closable: false },
      ],
    });
    const details = cardDetails(
      card,
      {
        text: "Running checks",
        since: "1 min ago",
        stalled: false,
        run: "cr-4",
        files: 3,
      },
      10,
    );
    expect(details.rows.map((row) => row.name)).toEqual([
      "Worker",
      "Kind",
      "Blocker",
      "Needs",
      "Blocks",
      "Proposal",
      "Report",
      "Checks",
      "Files",
      "Messages",
      "Human review",
      "Checklist",
      "Labels",
    ]);
    expect(details.rows).toContainEqual({
      kind: "text",
      name: "Needs",
      text: "T-2, T-9",
    });
    expect(details.rows).toContainEqual({
      kind: "text",
      name: "Blocks",
      text: "1 task",
    });
    expect(details.rows).toContainEqual({
      kind: "proposal",
      name: "Proposal",
      text: "P-4 · waiting for a batch",
      look: "active",
    });
    expect(details.rows).toContainEqual({
      kind: "person",
      name: "Human review",
      person: "dev",
    });
    expect(details.rows).toContainEqual({
      kind: "text",
      name: "Checks",
      text: "cr-4 running",
    });
    expect(details.rows).toContainEqual({
      kind: "text",
      name: "Files",
      text: "3 files",
    });
    expect(details.why).toBe(
      "Stalled: its worker made no edits for a while · Stale claim: the worker's lease ran out.",
    );
    expect(details.notes).toEqual(["rp-2 waits for T-9 until the next check"]);
  });

  it("says any human may approve, and counts more than one blocked task", () => {
    const details = cardDetails(
      boardCard("T-1", {
        blocks: 3,
        human_review: { reviewer: null, reason: "Auth", set_by: "planner" },
        proposal: {
          id: "P-1",
          state: "approved",
          version: 2,
          checks_failed: false,
          revision: null,
          review_plan: null,
          review_claim: null,
        },
      }),
      null,
      0,
    );
    expect(details.rows).toContainEqual({
      kind: "text",
      name: "Blocks",
      text: "3 tasks",
    });
    expect(details.rows).toContainEqual({
      kind: "text",
      name: "Human review",
      text: "any human",
    });
    expect(details.notes).toEqual([]);
  });

  it("keeps a report of a task with a proposal out", () => {
    const details = cardDetails(
      boardCard("T-1", { report: "Old note" }),
      null,
      0,
    );
    expect(details.rows).toEqual([]);
  });

  it("shows a short report and blocker whole", () => {
    const details = cardDetails(
      boardCard("T-3", {
        kind: "no_proposal",
        report: "Drew the `icons`.",
        blocker: { reason: "Wait for the logo", set_by: "tanim", set_at: 0 },
      }),
      null,
      0,
    );
    expect(details.rows.filter((row) => row.kind === "prose")).toEqual([
      {
        kind: "prose",
        name: "Blocker",
        text: "Wait for the logo",
        clamp: null,
        more: null,
      },
      {
        kind: "prose",
        name: "Report",
        text: "Drew the `icons`.",
        clamp: null,
        more: null,
      },
    ]);
  });
});

describe("proseRow", () => {
  it("clamps a text of more lines than the panel shows and sends the person to the task", () => {
    const text = Array.from({ length: PREVIEW_LINES + 1 }, (_, i) =>
      String(i),
    ).join("\n");
    expect(proseRow("Report", text, "report")).toEqual({
      kind: "prose",
      name: "Report",
      text,
      clamp: PREVIEW_LINES,
      more: "Open the task to read the full report",
    });
  });

  it("clamps one long paragraph, and counts neither blank lines nor the spaces around the text", () => {
    expect(
      proseRow("Blocker", "x".repeat(PREVIEW_CHARS + 1), "reason").more,
    ).toBe("Open the task to read the full reason");
    const four = Array.from({ length: PREVIEW_LINES }, (_, i) =>
      String(i),
    ).join("\n\n");
    expect(proseRow("Report", `  ${four}\n\n  `, "report").clamp).toBeNull();
    expect(
      proseRow("Report", "y".repeat(PREVIEW_CHARS), "report").clamp,
    ).toBeNull();
  });
});
