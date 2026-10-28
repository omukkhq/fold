import { describe, expect, it } from "vitest";
import { INITIAL_STATUS, type LiveStatus } from "../live/status";
import { streamLabel, streamView, syncedAt } from "./streamStatusModel";

const status = (patch: Partial<LiveStatus>): LiveStatus => ({
  ...INITIAL_STATUS,
  ...patch,
});

describe("syncedAt", () => {
  it("is null before anything arrived", () => {
    expect(syncedAt(INITIAL_STATUS)).toBeNull();
  });

  it("takes the newest of the event and the snapshot", () => {
    expect(syncedAt(status({ lastEventAt: 5, stateAsOf: 9 }))).toBe(9);
    expect(syncedAt(status({ lastEventAt: 7 }))).toBe(7);
    expect(syncedAt(status({ stateAsOf: 3 }))).toBe(3);
  });
});

describe("streamView", () => {
  it("shows connecting with a faint dot", () => {
    expect(streamView(INITIAL_STATUS, 0)).toEqual({
      label: "Connecting…",
      detail: null,
      dot: "bg-text-2",
      tone: "text-text-2",
    });
  });

  it("shows live with the time since the last sync", () => {
    const view = streamView(status({ state: "live", lastEventAt: 1000 }), 3000);
    expect(view.label).toBe("Live");
    expect(view.detail).toBe("synced 2 s ago");
    expect(view.dot).toBe("bg-ok");
  });

  it("shows live without a time before the first event", () => {
    expect(streamView(status({ state: "live" }), 0).detail).toBe("synced");
  });

  it("shows reconnecting in the secondary colour", () => {
    expect(streamView(status({ state: "reconnecting" }), 0)).toEqual({
      label: "Reconnecting…",
      detail: null,
      dot: "bg-warn",
      tone: "text-warn",
    });
  });
});

describe("streamLabel", () => {
  it("names each connection state as the stream status shows it", () => {
    expect(streamLabel("connecting")).toBe("Connecting…");
    expect(streamLabel("reconnecting")).toBe("Reconnecting…");
    expect(streamLabel("live")).toBe("Live");
    expect(streamView(status({ state: "live" }), 0).label).toBe(
      streamLabel("live"),
    );
  });
});
