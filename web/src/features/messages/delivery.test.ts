import { describe, expect, it } from "vitest";
import { message as baseMessage, recipient } from "@/test/messages";
import {
  deliveryLines,
  previewLine,
  recipientNames,
  recipientState,
  routeLine,
} from "./delivery";

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
    expect(states[0]).toMatch(
      /^acknowledged by frontend\/ui · \d\d:\d\d:\d\d$/,
    );
    expect(states[1]).toMatch(/^delivered to backend · \d\d:\d\d:\d\d$/);
    expect(states[2]).toBe("not delivered to review");
    expect(states[3]).toMatch(/^read by tanim · \d\d:\d\d:\d\d$/);
  });
  it("has a line per recipient", () => {
    const lines = deliveryLines(message, NOW);
    expect(lines).toHaveLength(4);
    expect(lines[2]).toBe("not delivered to review");
  });
  it("words the route and the preview without the human prefix", () => {
    expect(routeLine(message)).toBe(
      "tanim → frontend/ui, backend, review, tanim",
    );
    expect(recipientNames(message)).toBe("frontend/ui, backend, review, tanim");
    expect(previewLine(message)).toBe("tanim: Hello");
  });
});
