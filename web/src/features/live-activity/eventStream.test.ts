import { describe, expect, it } from "vitest";
import type { DurableEvent } from "../../api/events";
import { EVENT_LIMIT } from "../../live/eventList";
import { checkoutRows, checkoutText, eventRows, liveDot } from "./eventStream";

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
  it("joins the proposal, version, checkout path and last command result", () => {
    const rows = checkoutRows(
      [
        {
          workspace: "ws-12",
          path: "ws/review-p-4",
          proposal: "P-4",
          version: 2,
          worker: "review/quality",
          base: "r7",
          created_at: NOW,
          last_command: {
            result: "ran cargo nextest, 48 passed",
            at: NOW - 120_000,
          },
        },
        {
          workspace: "ws-13",
          path: "ws/review-p-5",
          proposal: "P-5",
          version: 1,
          worker: null,
          base: "r7",
          created_at: NOW,
        },
      ],
      NOW,
    );
    expect(rows[0]).toEqual({
      workspace: "ws-12",
      worker: "review/quality",
      where: "P-4 v2 in ws/review-p-4",
      last: "ran cargo nextest, 48 passed · 2 min ago",
    });
    expect(rows[1]).toMatchObject({ worker: null, last: null });
    const [none] = checkoutRows(
      [
        {
          workspace: "ws-14",
          path: "ws/review-p-6",
          proposal: "P-6",
          version: 1,
          worker: "review",
          base: "r7",
          created_at: NOW,
          last_command: null,
        },
      ],
      NOW,
    );
    expect(none?.last).toBe("no command yet");
  });

  it("leaves out an unknown last command, as the daemon records none", () => {
    expect(
      checkoutText({
        workspace: "ws-13",
        worker: "review",
        where: "P-5 v1 in ws/review-p-5",
        last: null,
      }),
    ).toBe("P-5 v1 in ws/review-p-5 · not recordable");
    expect(
      checkoutText({
        workspace: "ws-12",
        worker: "review",
        where: "P-4 v2 in ws/review-p-4",
        last: "ran tests · 2 min ago",
      }),
    ).toBe("P-4 v2 in ws/review-p-4 · ran tests · 2 min ago · not recordable");
  });
});
