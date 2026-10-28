import { describe, expect, it } from "vitest";
import { type PersonSlot, slotSize } from "./sizes";

const FRAMES: [PersonSlot, string, string][] = [
  ["messageHead", "text-13", "text-14"],
  ["requester", "text-14", "text-15"],
  ["gate", "text-14", "text-15"],
  ["waitingChip", "text-13", "text-14"],
  ["eventActor", "text-13", "text-14"],
  ["authorCell", "text-14", "text-14"],
  ["waitingLine", "text-13", "text-14"],
  ["foot", "text-14", "text-14"],
  ["teamRow", "text-15", "text-14"],
  ["teamNote", "text-16", "text-14"],
  ["needsYouLine", "text-15", "text-15"],
  ["panelName", "text-16", "text-16"],
  ["answeredBy", "text-15", "text-15"],
  ["batchHead", "text-16", "text-16"],
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
