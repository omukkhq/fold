import { describe, expect, it } from "vitest";
import { message as baseMessage, recipient } from "@/test/messages";
import {
  deliveryLines,
  previewLine as previewLineSegments,
  recipientActors,
  recipientState,
  routeLine as routeLineSegments,
  unreadBy,
} from "./delivery";
import { formatAsOf } from "@/format/time";
import { segmentsText } from "@/people/segments";

const routeLine = (...args: Parameters<typeof routeLineSegments>) =>
  segmentsText(routeLineSegments(...args));

const previewLine = (...args: Parameters<typeof previewLineSegments>) =>
  segmentsText(previewLineSegments(...args));

const NOW = Date.UTC(2026, 9, 1, 12, 0, 0);

const message = baseMessage("m-1", {
  sender: "human:tanim",
  recipients: [
    recipient("frontend/ui", { delivered_at: NOW, acknowledged_at: NOW }),
    recipient("backend", { delivered_at: NOW }),
    recipient("review"),
    recipient("human:tanim", {
      addressed_as: "human",
      delivered_at: NOW,
      acknowledged_at: NOW,
    }),
  ],
  body: "Hello",
  sent_at: NOW,
});

describe("delivery", () => {
  it("shows acknowledgement before delivery before neither", () => {
    const states = message.recipients.map((r) => recipientState(r, NOW));
    expect(states[0]).toMatchObject({
      lead: "acknowledged by",
      actor: "frontend/ui",
    });
    expect(states[0]?.at).toMatch(/^\d\d:\d\d:\d\d$/);
    expect(states[1]).toMatchObject({ lead: "delivered to", actor: "backend" });
    expect(states[2]).toEqual({
      lead: "not delivered to",
      actor: "review",
      at: null,
      note: null,
    });
    expect(states[3]).toMatchObject({ lead: "read by", actor: "human:tanim" });
  });
  it("says a human's message was delivered when it was sent until the human reads it", () => {
    const sent = NOW - 60_000;
    const human = recipientState(recipient("human:rafeed"), NOW, null, sent);
    expect(human).toMatchObject({
      lead: "delivered to",
      actor: "human:rafeed",
      note: null,
    });
    expect(human.at).toBe(formatAsOf(NOW, sent));
    expect(recipientState(recipient("human:rafeed"), NOW).at).toBe(
      formatAsOf(NOW, NOW),
    );
    expect(
      recipientState(recipient("human:rafeed", { acknowledged_at: NOW }), NOW),
    ).toMatchObject({ lead: "read by", actor: "human:rafeed" });
  });

  it("never says not delivered to a human", () => {
    const lines = deliveryLines(
      baseMessage("m-2", {
        sent_at: NOW,
        recipients: [
          recipient("human:rafeed", { reachable: false }),
          recipient("review"),
        ],
      }),
      NOW,
    );
    expect(lines.map((line) => line.lead)).toEqual([
      "delivered to",
      "not delivered to",
    ]);
  });

  it("finds the newest message that waits for the signed-in human", () => {
    const thread = [
      baseMessage("m-1", { recipients: [recipient("human:tanim")] }),
      baseMessage("m-2", {
        recipients: [recipient("human:ana"), recipient("human:tanim")],
      }),
      baseMessage("m-3", {
        recipients: [recipient("human:tanim", { acknowledged_at: NOW })],
      }),
      baseMessage("m-4", { recipients: [recipient("human:ana")] }),
    ];
    expect(unreadBy(thread, "tanim")).toBe("m-2");
    expect(unreadBy(thread, "rafeed")).toBeNull();
    expect(unreadBy(thread, null)).toBeNull();
  });

  it("says when fold cannot tell an agent that the message waits", () => {
    const unreachable = recipient("main", { reachable: false });
    expect(recipientState(unreachable, NOW)).toEqual({
      lead: "not delivered to",
      actor: "main",
      at: null,
      note: "not reachable now · it gets the message when it reads its inbox",
    });
    expect(
      recipientState(recipient("main", { reachable: true }), NOW).note,
    ).toBe(null);
    expect(
      recipientState(
        recipient("main", { reachable: false, delivered_at: NOW }),
        NOW,
      ).note,
    ).toBe(null);
  });
  it("says that a message waits in a busy agent's session once fold pushed its notice", () => {
    const waiting = recipient("backend-lead", { pushed_at: NOW });
    const state = recipientState(waiting, NOW);
    expect(state).toMatchObject({
      lead: "waiting for",
      actor: "backend-lead",
      note: "it is busy until its current step ends",
    });
    expect(state.at).toMatch(/^\d\d:\d\d:\d\d$/);
    expect(
      recipientState(
        recipient("backend-lead", { pushed_at: NOW, reachable: false }),
        NOW,
      ),
    ).toMatchObject({
      lead: "waiting for",
      note: "not reachable now · it gets the message when it reads its inbox",
    });
    expect(
      recipientState(
        recipient("backend-lead", { pushed_at: NOW, reachable: true }),
        NOW,
      ).note,
    ).toBe("it is busy until its current step ends");
    expect(
      recipientState(
        recipient("backend-lead", { pushed_at: NOW, delivered_at: NOW }),
        NOW,
      ).lead,
    ).toBe("delivered to");
    expect(
      recipientState(
        recipient("backend-lead", {
          pushed_at: NOW,
          delivered_at: NOW,
          acknowledged_at: NOW,
        }),
        NOW,
      ).lead,
    ).toBe("acknowledged by");
  });
  it("names the lead that got the message of a task worker that could not start", () => {
    const forwarded = recipient("backend-worker-10", {
      forwarded_to: "backend-lead",
    });
    expect(recipientState(forwarded, NOW, "human:tanim")).toEqual({
      lead: "",
      actor: "backend-worker-10",
      at: null,
      note: "cannot start now · sent on to @backend-lead",
    });
    expect(recipientState(forwarded, NOW, "backend-lead").note).toBe(
      "cannot start now",
    );
    expect(recipientState({ ...forwarded, delivered_at: NOW }, NOW).lead).toBe(
      "delivered to",
    );
    const lines = deliveryLines(
      baseMessage("m-221", {
        sender: "human:tanim",
        recipients: [forwarded, recipient("backend-lead")],
      }),
      NOW,
    );
    expect(lines.map((line) => [line.lead, line.actor])).toEqual([
      ["", "backend-worker-10"],
      ["not delivered to", "backend-lead"],
    ]);
  });
  it("has a line per recipient", () => {
    const lines = deliveryLines(message, NOW);
    expect(lines).toHaveLength(4);
    expect(lines[2]?.lead).toBe("not delivered to");
    expect(recipientActors(message)).toEqual([
      "frontend/ui",
      "backend",
      "review",
      "human:tanim",
    ]);
  });
  it("words the route and the preview as @names", () => {
    expect(routeLine(message)).toBe(
      "@tanim → @frontend/ui, @backend, @review, @tanim",
    );
    expect(previewLine(message)).toBe("@tanim: Hello");
  });
});
