import { describe, expect, it } from "vitest";
import { isEventEnvelope, isSnapshot } from "./envelope";

describe("isEventEnvelope", () => {
  it("accepts an event", () => {
    expect(
      isEventEnvelope({
        seq: 3,
        type: "task.status_changed",
        task: 1,
        data: { status: "done" },
      }),
    ).toBe(true);
  });

  it("accepts an event of an unknown type", () => {
    expect(isEventEnvelope({ seq: 3, type: "future.thing" })).toBe(true);
  });

  it("accepts a snapshot", () => {
    const snapshot = {
      seq: 9,
      type: "snapshot",
      workspaces: [{ workspace: 1, state: { changed: 1 } }],
    };
    expect(isEventEnvelope(snapshot)).toBe(true);
    expect(isSnapshot(snapshot)).toBe(true);
  });

  it("refuses a frame with no seq or no type", () => {
    expect(isEventEnvelope({ type: "task.created" })).toBe(false);
    expect(isEventEnvelope({ seq: 1 })).toBe(false);
    expect(isEventEnvelope({ seq: 1, type: "" })).toBe(false);
  });

  it("refuses wrong field types", () => {
    for (const bad of [
      null,
      "x",
      [],
      { seq: "1", type: "a.b" },
      { seq: 1.5, type: "a.b" },
      { seq: -1, type: "a.b" },
      { seq: 1, type: "a.b", task: "T-1" },
      { seq: 1, type: "a.b", data: [] },
      { seq: 1, type: "snapshot" },
      { seq: 1, type: "snapshot", workspaces: [{ workspace: "1", state: {} }] },
    ]) {
      expect(isEventEnvelope(bad)).toBe(false);
    }
  });

  it("does not treat a plain event as a snapshot", () => {
    expect(isSnapshot({ seq: 1, type: "snapshot" })).toBe(false);
    expect(isSnapshot({ seq: 1, type: "task.created" })).toBe(false);
  });
});
