import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import {
  createLiveStatusStore,
  INITIAL_STATUS,
  liveStatusStore,
  useLiveStatus,
} from "./status";

describe("live status store", () => {
  it("starts connecting and notifies on a change only", () => {
    const store = createLiveStatusStore();
    const listener = vi.fn();
    const off = store.subscribe(listener);
    expect(store.getSnapshot()).toEqual(INITIAL_STATUS);
    store.set({ state: "live" });
    store.set({ state: "live" });
    expect(listener).toHaveBeenCalledTimes(1);
    store.set({ lastEventAt: 5 });
    store.set({ stateAsOf: 6 });
    expect(store.getSnapshot()).toEqual({
      state: "live",
      lastEventAt: 5,
      stateAsOf: 6,
    });
    off();
    store.reset();
    expect(listener).toHaveBeenCalledTimes(3);
    expect(store.getSnapshot()).toEqual(INITIAL_STATUS);
  });

  it("is read by useLiveStatus", () => {
    liveStatusStore.set({ state: "reconnecting" });
    function Probe() {
      return createElement("p", null, useLiveStatus().state);
    }
    expect(renderToString(createElement(Probe))).toContain("reconnecting");
    liveStatusStore.reset();
  });
});
