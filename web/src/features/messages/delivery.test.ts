import { describe, expect, it } from "vitest";
import { message as baseMessage, recipient } from "@/test/messages";
import {
  deliveryLines,
  previewLine as previewLineSegments,
  recipientActors,
  recipientState,
  routeLine as routeLineSegments,
} from "./delivery";
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
