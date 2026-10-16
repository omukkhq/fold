import { describe, expect, it } from "vitest";
import { pauseControl, pausedBanner } from "./pause";

const AT = new Date(2026, 9, 17, 23, 40).getTime();

describe("pauseControl", () => {
  it("shows nothing until the project is read", () => {
    expect(pauseControl(undefined, null)).toBeNull();
  });

  it("offers a pause while the agents run and a resume while they are paused", () => {
    expect(pauseControl(null, null)).toEqual({
      action: "pause",
      label: "Pause agents",
      paused: false,
      pending: false,
    });
    expect(pauseControl(AT, null)).toEqual({
      action: "resume",
      label: "Resume agents",
      paused: true,
      pending: false,
    });
  });

  it("stays pending until the daemon's state shows the action done", () => {
    expect(pauseControl(null, "pause")).toEqual({
      action: "pause",
      label: "Pausing…",
      paused: false,
      pending: true,
    });
    expect(pauseControl(AT, "resume")).toEqual({
      action: "resume",
      label: "Resuming…",
      paused: true,
      pending: true,
    });
    expect(pauseControl(AT, "pause")?.pending).toBe(false);
    expect(pauseControl(null, "resume")?.label).toBe("Pause agents");
  });
});

describe("pausedBanner", () => {
  it("names the time and who paused, and says nothing while the agents run", () => {
    expect(pausedBanner(AT + 60_000, null, null)).toBeNull();
    expect(pausedBanner(AT + 60_000, AT, "human:ana")?.title).toBe(
      "Agents paused since 23:40 by @ana",
    );
    expect(pausedBanner(AT + 60_000, AT, "frontend")?.title).toBe(
      "Agents paused since 23:40 by @frontend",
    );
    expect(pausedBanner(AT + 60_000, AT, null)?.title).toBe(
      "Agents paused since 23:40",
    );
    expect(pausedBanner(AT, AT, "frontend")?.detail).toContain(
      "Claims keep their leases.",
    );
  });
});
