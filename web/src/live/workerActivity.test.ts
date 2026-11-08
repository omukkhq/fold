import { describe, expect, it } from "vitest";
import {
  activityText,
  activityVerb,
  agentOf,
  isStall,
  toldText,
  workerActivity,
  type LiveActivity,
} from "./workerActivity";

const NOW = 1_800_000_000_000;
const MINUTE = 60_000;

function seen(over: Partial<LiveActivity> = {}): LiveActivity {
  return {
    state: "working",
    slot: null,
    call: "command",
    since: NOW - 8 * MINUTE,
    supervised: true,
    told: [],
    ...over,
  };
}

describe("activityText", () => {
  it("says what the worker does while a call is open", () => {
    expect(activityText(seen(), "web-worker", NOW)).toBe(
      "Running a command for 8 min",
    );
    expect(activityText(seen({ call: "fold" }), "web-worker", NOW)).toBe(
      "Waiting on fold for 8 min",
    );
    expect(activityText(seen({ call: "subagent" }), "web-worker", NOW)).toBe(
      "Running a subagent for 8 min",
    );
    expect(activityText(seen({ call: null }), "web-worker", NOW)).toBe(
      "Running a tool for 8 min",
    );
  });

  it("says what fold saw of a stall", () => {
    const quiet = seen({
      state: "silent",
      slot: null,
      call: null,
      since: NOW - 12 * MINUTE,
    });
    expect(activityText(quiet, "web-worker", NOW)).toBe(
      "No activity from @web-worker for 12 min",
    );
    const long = seen({ state: "long_call", since: NOW - 25 * MINUTE });
    expect(activityText(long, "web-worker", NOW)).toBe(
      "A command of @web-worker has run for 25 min",
    );
    expect(activityText({ ...long, call: "tool" }, "web-worker", NOW)).toBe(
      "A tool call of @web-worker has run for 25 min",
    );
    expect(
      activityText(seen({ state: "stopped", call: null }), "web-worker", NOW),
    ).toBe("@web-worker stopped");
  });

  it("says it after the worker's name in the task strip", () => {
    expect(activityVerb(seen(), NOW)).toBe("running a command for 8 min");
    expect(activityVerb(seen({ state: "silent", call: null }), NOW)).toBe(
      "no activity for 8 min",
    );
    expect(activityVerb(seen({ state: "long_call" }), NOW)).toBe(
      "a command has run for 8 min",
    );
    expect(activityVerb(seen({ state: "stopped", call: null }), NOW)).toBe(
      "stopped",
    );
  });
});

describe("a resume that waits for a free slot", () => {
  const waits = seen({
    state: "waits_for_slot",
    call: null,
    slot: "task_worker",
    since: NOW - 30 * MINUTE,
  });

  it("says which slot it waits for, with no time", () => {
    expect(activityText(waits, "web-worker", NOW)).toBe(
      "Waits for a free task worker slot",
    );
    expect(activityText({ ...waits, slot: "reviewer" }, "review", NOW)).toBe(
      "Waits for a free reviewer slot",
    );
    expect(activityVerb(waits, NOW)).toBe("waits for a free task worker slot");
  });

  it("is neutral: never a stall, never a person's, never a restart", () => {
    expect(isStall("waits_for_slot")).toBe(false);
    expect(isStall("working")).toBe(false);
    expect(isStall("silent")).toBe(true);
    const view = workerActivity(
      { activity: waits, stalled: true, worker: "web-worker" },
      NOW,
    );
    expect(view).toMatchObject({
      text: "Waits for a free task worker slot",
      stall: false,
      needsYou: false,
      restart: null,
    });
    expect(view === null ? "" : toldText(view)).toBeNull();
  });
});

describe("workerActivity", () => {
  it("is null without a frame or without anything to say", () => {
    expect(workerActivity(undefined, NOW)).toBeNull();
    expect(workerActivity(null, NOW)).toBeNull();
    expect(
      workerActivity({ activity: null, stalled: false, worker: "w" }, NOW),
    ).toBeNull();
  });

  it("is never a stall or a restart while the worker works", () => {
    const working = workerActivity(
      { activity: seen(), stalled: true, worker: "web-worker" },
      NOW,
    );
    expect(working).toMatchObject({
      stall: false,
      needsYou: false,
      restart: null,
    });
  });

  it("waits for a person only once the daemon flags the stall, and restarts only a worker fold runs", () => {
    const silent = seen({ state: "silent", call: null, told: ["web-lead"] });
    const told = workerActivity(
      { activity: silent, stalled: false, worker: "web-worker" },
      NOW,
    );
    expect(told).toMatchObject({
      stall: true,
      needsYou: false,
      restart: "web-worker",
      told: ["web-lead"],
      since: silent.since,
    });
    const flagged = workerActivity(
      { activity: silent, stalled: true, worker: "frontend/ui" },
      NOW,
    );
    expect(flagged).toMatchObject({ needsYou: true, restart: "frontend" });
    expect(
      workerActivity(
        {
          activity: { ...silent, supervised: false },
          stalled: true,
          worker: "frontend",
        },
        NOW,
      )?.restart,
    ).toBeNull();
  });
});

describe("toldText", () => {
  const base = {
    text: "",
    stall: true,
    needsYou: false,
    restart: null,
    since: 0,
  };

  it("names whom fold told, and says when a person must act", () => {
    expect(toldText({ ...base, told: ["web-lead"] })).toBe(
      "fold told @web-lead, who can start it again",
    );
    expect(toldText({ ...base, needsYou: true, told: ["web-lead"] })).toBe(
      "fold told @web-lead; it did not start again, so it waits for you",
    );
    expect(toldText({ ...base, needsYou: true, told: [] })).toBe(
      "No agent can act on it, so it waits for you",
    );
    expect(toldText({ ...base, stall: false, told: [] })).toBeNull();
  });
});

describe("agentOf", () => {
  it("is the agent of a worker label", () => {
    expect(agentOf("frontend/ui")).toBe("frontend");
    expect(agentOf("web-worker")).toBe("web-worker");
  });
});
