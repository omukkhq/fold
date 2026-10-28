import { describe, expect, it } from "vitest";
import { segmentsText } from "@/people/segments";
import { lastPauseWrite, pauseControl, pausedBanner, pauseNote } from "./pause";

const AT = new Date(2026, 9, 17, 23, 40).getTime();

describe("pauseControl", () => {
  it("shows nothing until the project is read", () => {
    expect(pauseControl(undefined, null)).toBeNull();
  });

  it("offers a pause while the agents run and a resume while they are paused", () => {
    expect(pauseControl(null, null)).toEqual({
      action: "pause",
      label: "Pause agents",
      pendingLabel: "Pausing…",
      paused: false,
      pending: false,
    });
    expect(pauseControl(AT, null)).toEqual({
      action: "resume",
      label: "Resume agents",
      pendingLabel: "Resuming…",
      paused: true,
      pending: false,
    });
  });

  it("stays pending until the daemon's state shows the action done", () => {
    expect(pauseControl(null, "pause")).toEqual({
      action: "pause",
      label: "Pause agents",
      pendingLabel: "Pausing…",
      paused: false,
      pending: true,
    });
    expect(pauseControl(AT, "resume")).toEqual({
      action: "resume",
      label: "Resume agents",
      pendingLabel: "Resuming…",
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
    expect(
      segmentsText(pausedBanner(AT + 60_000, AT, "human:ana")?.title ?? []),
    ).toBe("Agents paused since 23:40 by @ana");
    expect(
      segmentsText(pausedBanner(AT + 60_000, AT, "frontend")?.title ?? []),
    ).toBe("Agents paused since 23:40 by @frontend");
    expect(pausedBanner(AT + 60_000, AT, "frontend")?.title).toContainEqual({
      actor: "frontend",
    });
    expect(segmentsText(pausedBanner(AT + 60_000, AT, null)?.title ?? [])).toBe(
      "Agents paused since 23:40",
    );
    expect(pausedBanner(AT, AT, "frontend")?.detail).toContain(
      "Claims keep their leases.",
    );
  });
});

describe("lastPauseWrite", () => {
  it("is idle before any write", () => {
    expect(lastPauseWrite([])).toEqual({ sent: null, error: null });
  });

  it("reads the action in flight and the error of the newest write only", () => {
    const failed = new Error("refused");
    expect(
      lastPauseWrite([
        { status: "error", variables: "pause", error: failed },
        { status: "pending", variables: "resume", error: null },
      ]),
    ).toEqual({ sent: "resume", error: null });
    expect(
      lastPauseWrite([
        { status: "success", variables: "pause", error: null },
        { status: "error", variables: "resume", error: failed },
      ]),
    ).toEqual({ sent: null, error: failed });
    expect(
      lastPauseWrite([{ status: "success", variables: "pause", error: null }]),
    ).toEqual({ sent: null, error: null });
  });

  it("takes no action from a write it does not know", () => {
    expect(
      lastPauseWrite([{ status: "pending", variables: "stop", error: null }]),
    ).toEqual({ sent: null, error: null });
  });
});

describe("pauseNote", () => {
  it("says what a pause keeps, and what a resume starts", () => {
    expect(pauseNote(false)).toMatch(/^Pause stops every worker/);
    expect(pauseNote(true)).toMatch(/Resume starts the workers again\.$/);
  });
});
