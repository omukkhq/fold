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
];

describe("slotSize", () => {
  it.each(FRAMES)(
    "%s draws an agent and a human at their frame sizes",
    (slot, agent, human) => {
      expect(slotSize(slot, "agent")).toBe(agent);
      expect(slotSize(slot, "human")).toBe(human);
    },
  );
});
