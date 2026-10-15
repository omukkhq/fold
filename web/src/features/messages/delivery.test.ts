import { describe, expect, it } from "vitest";
import type { MessageView } from "@/api/messages";
import {
  deliveryLines,
  previewLine,
  recipientNames,
  recipientState,
  routeLine,
} from "./delivery";

const NOW = Date.UTC(2026, 9, 1, 12, 0, 0);

const message: MessageView = {
  id: "m-1",
  thread: "m-1",
  sender: "human:tanim",
  recipients: [
    { name: "frontend/ui", delivered_at: NOW, acknowledged_at: NOW },
    { name: "backend", delivered_at: NOW, acknowledged_at: null },
    { name: "review", delivered_at: null, acknowledged_at: null },
    { name: "human:tanim", delivered_at: NOW, acknowledged_at: NOW },
  ],
  body: "Hello",
  sent_at: NOW,
  reply_to: null,
};

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
