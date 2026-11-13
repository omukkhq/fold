import { formatAge } from "@/format/time";
import type { components } from "../api/schema";
import type { LiveWorkspace } from "./envelope";

export type LiveActivity = components["schemas"]["LiveActivity"];
type LiveCall = components["schemas"]["LiveCall"];

/** What the board, the task, Activity and Needs you show of a workspace's worker, from the daemon's live state alone. */
export interface WorkerActivity {
  /** The state the daemon decided, which the chip draws its icon from. */
  state: LiveActivity["state"];
  /** `Running a command for 8 min`, `No activity from @web-worker for 12 min`, `A command of @web-worker has run for 25 min`, `@web-worker stopped`, `@web-worker failed`, `Waiting for a free slot`, `Waiting for the network` or `Waits for the usage limit reset`. */
  text: string;
  /** Silent, a call that ran long, or a process that failed: someone must act. Work in progress, a process that was stopped on purpose, a resume that waits for a free slot and a turn that waits for the network or a usage limit, which fold runs again, are never a stall. */
  stall: boolean;
  /** The stall waits for a person: fold told the lead and it lasted one more window, or no agent can act. */
  needsYou: boolean;
  /** The agent that Restart worker starts again in a new session, once the stall waits for a person; null before that, while it works, or when fold does not run its session. */
  restart: string | null;
  /** The agents fold told of the stall. */
  told: readonly string[];
  /** UTC milliseconds since what the text names. */
  since: number;
}

const WORKING: Readonly<Record<LiveCall, string>> = {
  command: "Running a command",
  fold: "Waiting on fold",
  subagent: "Running a subagent",
  question: "Waiting for an answer",
  tool: "Running a tool",
};

/** One text for task workers and reviewers: the board's column says which place the worker waits for. */
const SLOT_WAIT = "Waiting for a free slot";

/** A turn that could not reach the model, which fold runs again once the model answers. */
export const NETWORK_WAIT = "Waiting for the network";

/** A turn that reached a usage limit, which fold runs again after the reset. */
export const USAGE_LIMIT_WAIT = "Waits for the usage limit reset";

const LONG: Readonly<Record<LiveCall, string>> = {
  command: "A command",
  fold: "A fold call",
  subagent: "A subagent",
  question: "A question",
  tool: "A tool call",
};

/** The agent of a worker label: `frontend/ui` is a worker of `frontend`. */
export function agentOf(worker: string): string {
  return worker.split("/")[0] ?? worker;
}

/** What fold saw, in the words of the chip. */
export function activityText(
  activity: LiveActivity,
  worker: string,
  now: number,
): string {
  const time = formatAge(now, activity.since);
  const who = `@${worker}`;
  switch (activity.state) {
    case "working":
      return `${WORKING[activity.call ?? "tool"]} for ${time}`;
    case "silent":
      return `No activity from ${who} for ${time}`;
    case "long_call":
      return `${LONG[activity.call ?? "tool"]} of ${who} has run for ${time}`;
    case "stopped":
      return `${who} stopped`;
    case "failed":
      return `${who} failed`;
    case "waits_for_slot":
      return SLOT_WAIT;
    case "waits_for_network":
      return NETWORK_WAIT;
    case "waits_for_usage_limit":
      return USAGE_LIMIT_WAIT;
  }
}

/** What fold saw, after the worker's name: `running a command for 8 min`, `no activity for 12 min`, `a command has run for 25 min`, `stopped`, `failed`, `waiting for a free slot`, `waiting for the network` or `waits for the usage limit reset`. */
export function activityVerb(activity: LiveActivity, now: number): string {
  const time = formatAge(now, activity.since);
  switch (activity.state) {
    case "working": {
      const what = WORKING[activity.call ?? "tool"];
      return `${what.charAt(0).toLowerCase()}${what.slice(1)} for ${time}`;
    }
    case "silent":
      return `no activity for ${time}`;
    case "long_call": {
      const what = LONG[activity.call ?? "tool"];
      return `${what.charAt(0).toLowerCase()}${what.slice(1)} has run for ${time}`;
    }
    case "stopped":
      return "stopped";
    case "failed":
      return "failed";
    case "waits_for_slot":
      return "waiting for a free slot";
    case "waits_for_network":
      return "waiting for the network";
    case "waits_for_usage_limit":
      return "waits for the usage limit reset";
  }
}

/** The states that never need anyone to act: work, a stop that someone asked for, such as a person's Stop or a pause, and the waits that fold ends itself. */
const NOT_STALLS: ReadonlySet<LiveActivity["state"]> = new Set([
  "working",
  "stopped",
  "waits_for_slot",
  "waits_for_network",
  "waits_for_usage_limit",
]);

/** Whether `state` needs someone to act. */
export function isStall(state: LiveActivity["state"]): boolean {
  return !NOT_STALLS.has(state);
}

/** The worker's activity in `live`, or null while there is nothing to say. */
export function workerActivity(
  live:
    | Pick<LiveWorkspace, "activity" | "stalled" | "worker">
    | null
    | undefined,
  now: number,
): WorkerActivity | null {
  const activity = live?.activity ?? null;
  if (live === null || live === undefined || activity === null) {
    return null;
  }
  const stall = isStall(activity.state);
  const needsYou = stall && live.stalled;
  return {
    state: activity.state,
    text: activityText(activity, live.worker, now),
    stall,
    needsYou,
    // The lead fold told gets one stall window to restart the worker itself; only then does a person get the button.
    restart: needsYou && activity.supervised ? agentOf(live.worker) : null,
    told: activity.told,
    since: activity.since,
  };
}

/** `fold told @web-lead`, or that no agent can act; null while the worker works. */
export function toldText(activity: WorkerActivity): string | null {
  if (!activity.stall) {
    return null;
  }
  if (activity.told.length === 0) {
    return "No agent can act on it, so it waits for you";
  }
  const names = activity.told.map((name) => `@${name}`).join(", ");
  return activity.needsYou
    ? `fold told ${names}; it did not start again, so it waits for you`
    : `fold told ${names}, who can start it again`;
}
