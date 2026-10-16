import { describe, expect, it } from "vitest";
import type { DurableEvent } from "../../api/events";
import { EVENT_LIMIT } from "../../live/eventList";
import {
  checkoutRows,
  checkoutText,
  commitsByRevision,
  eventRows,
  liveDot,
} from "./eventStream";

function event(seq: number, over: Partial<DurableEvent> = {}): DurableEvent {
  return {
    seq,
    type: "task.created",
    actor: "fold",
    task: "T-1",
    proposal: null,
    workspace: null,
    revision: null,
    data: {},
    created_at: new Date(2026, 9, 1, 14, 12, 31).getTime(),
    ...over,
  };
}

const NOW = new Date(2026, 9, 1, 15, 0, 0).getTime();

describe("eventRows", () => {
  it("lists the newest event first with its actor, clock time and line", () => {
    const rows = eventRows([event(1), event(3, { actor: "backend/api" })], NOW);
    expect(rows.map((row) => row.seq)).toEqual([3, 1]);
    expect(rows[0]).toMatchObject({
      actor: "backend/api",
      time: "14:12:31",
      text: "created T-1",
      emphasis: false,
    });
  });

  it("shows no more than the limit", () => {
    const many = Array.from({ length: EVENT_LIMIT + 5 }, (_, i) => event(i));
    const rows = eventRows(many, NOW);
    expect(rows).toHaveLength(EVENT_LIMIT);
    expect(rows[0]?.seq).toBe(EVENT_LIMIT + 4);
  });

  it("gives an unknown type the generic line", () => {
    const unknown = { ...event(1), type: "future.thing" };
    expect(eventRows([unknown], NOW)[0]?.text).toBe("event future.thing");
  });

  it("adds the Git commit to the integration line of its revision and gives git.committed no row", () => {
    const rows = eventRows(
      [
        event(1, {
          type: "proposal.integrated",
          proposal: "P-5",
          revision: "r8",
        }),
        event(2, { type: "main.advanced", revision: "r8" }),
        event(3, {
          type: "git.committed",
          revision: "r8",
          data: { commit: "4f2a9c1e" },
        }),
      ],
      NOW,
    );
    expect(rows.map((row) => [row.seq, row.commit])).toEqual([
      [2, null],
      [1, "4f2a9c1e"],
    ]);
  });

  it("shows no commit on an integration line before its commit is written", () => {
    const rows = eventRows(
      [
        event(1, { type: "integration.succeeded", revision: "r9" }),
        event(2, { type: "integration.succeeded", revision: null }),
      ],
      NOW,
    );
    expect(rows.map((row) => row.commit)).toEqual([null, null]);
  });

  it("returns no rows for no events", () => {
    expect(eventRows([], NOW)).toEqual([]);
  });
});

describe("liveDot", () => {
  it("is live only while the stream is connected", () => {
    expect(liveDot("live")).toEqual({ label: "live", tone: "live" });
    expect(liveDot("connecting")).toEqual({
      label: "connecting",
      tone: "connecting",
    });
    expect(liveDot("reconnecting")).toEqual({
      label: "reconnecting",
      tone: "reconnecting",
    });
  });
});

describe("checkoutRows", () => {
  it("joins the proposal, version and checkout path", () => {
    const rows = checkoutRows([
      {
        workspace: "ws-12",
        path: "ws/review-p-4",
        target: { kind: "proposal", proposal: "P-4", version: 2 },
        worker: "review/quality",
        base: "r7",
        created_at: NOW,
      },
      {
        workspace: "ws-13",
        path: "ws/review-p-5",
        target: { kind: "proposal", proposal: "P-5", version: 1 },
        worker: null,
        base: "r7",
        created_at: NOW,
      },
    ]);
    expect(rows[0]).toEqual({
      workspace: "ws-12",
      worker: "review/quality",
      where: "P-4 v2 in ws/review-p-4",
    });
    expect(rows[1]).toMatchObject({ worker: null });
  });

  it("names a batch checkout's batch and its members in merge order", () => {
    const [batch] = checkoutRows([
      {
        workspace: "ws-15",
        path: "ws/review-rb-3",
        target: {
          kind: "batch",
          batch: "rb-3",
          members: [
            { proposal: "P-2", version: 1 },
            { proposal: "P-3", version: 2 },
          ],
        },
        worker: "review/batch",
        base: "r7",
        created_at: NOW,
      },
    ]);
    expect(batch?.where).toBe("rb-3 · P-2 v1 + P-3 v2 in ws/review-rb-3");
  });

  it("marks the checkout as not recordable", () => {
    expect(
      checkoutText({
        workspace: "ws-13",
        worker: "review",
        where: "P-5 v1 in ws/review-p-5",
      }),
    ).toBe("P-5 v1 in ws/review-p-5 · not recordable");
  });
});

describe("commitsByRevision", () => {
  it("maps each revision to its first reported commit and skips malformed events", () => {
    const committed = (seq: number, revision: string | null, data = {}) =>
      event(seq, { type: "git.committed", revision, data });
    const map = commitsByRevision([
      committed(1, "r1", { commit: "aaa" }),
      committed(2, "r1", { commit: "bbb" }),
      committed(3, null, { commit: "ccc" }),
      committed(4, "r2", { commit: 7 }),
      event(5, {
        type: "main.advanced",
        revision: "r3",
        data: { commit: "d" },
      }),
    ]);
    expect([...map]).toEqual([["r1", "aaa"]]);
  });
});
