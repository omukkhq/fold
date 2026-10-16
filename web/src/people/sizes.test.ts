import { describe, expect, it } from "vitest";
import { type PersonSlot, slotSize } from "./sizes";

const FRAMES: [PersonSlot, string, string][] = [
  ["messageHead", "text-meta", "text-bar"],
  ["requester", "text-bar", "text-body"],
  ["gate", "text-bar", "text-body"],
  ["waitingChip", "text-meta", "text-bar"],
  ["eventActor", "text-code", "text-bar"],
  ["authorCell", "text-bar", "text-bar"],
  ["waitingLine", "text-code", "text-bar"],
  ["foot", "text-bar", "text-bar"],
  ["teamRow", "text-body", "text-bar"],
  ["teamNote", "text-card", "text-bar"],
  ["needsYouLine", "text-body", "text-body"],
  ["panelName", "text-section", "text-section"],
  ["answeredBy", "text-body", "text-body"],
  ["batchHead", "text-card", "text-card"],
];

describe("slotSize", () => {
  it.each(FRAMES)(
    "%s draws an agent and a human at their slot sizes",
    (slot, agent, human) => {
      expect(slotSize(slot, "agent")).toBe(agent);
      expect(slotSize(slot, "human")).toBe(human);
    },
  );
});
