import type { components } from "@/api/schema";
import { formatAsOfMinute } from "@/format/time";

export type ReviewPlanWait = components["schemas"]["ReviewPlanWait"];

/** `waiting`: the plan still waits for tasks; `ready`: it waits only for the review lead to start one reviewer. */
export type PlanWaitState = "waiting" | "ready";

export interface PlanWait {
  state: PlanWaitState;
  /** The short state, for a pill. */
  label: string;
  /** The state as a heading, for the proposal page. */
  title: string;
  /** What the proposal waits for, naming the awaited tasks and the end of the wait, or the plan's other proposals. */
  detail: string;
}

const LABEL: Record<PlanWaitState, string> = {
  waiting: "waiting for a batch",
  ready: "batch ready",
};

const TITLE: Record<PlanWaitState, string> = {
  waiting: "Waiting for a batch",
  ready: "Batch ready",
};

/** A plan waits for its tasks until none is awaited or the wait ended at the deadline. */
export function planWaitState(plan: ReviewPlanWait): PlanWaitState {
  return plan.awaited.length > 0 && !plan.wait_ended ? "waiting" : "ready";
}

export function planWaitLabel(plan: ReviewPlanWait): string {
  return LABEL[planWaitState(plan)];
}

/**
 * How an open proposal in a review plan waits for its batch: for the awaited tasks until the deadline, or, once none is
 * awaited or the wait ended, for the review lead to start one reviewer. `self` is the proposal the page or card shows.
 * fold ends a wait at its first check after the deadline, so a deadline that `now` passed reads as the next check.
 */
export function planWait(
  plan: ReviewPlanWait | null,
  self: string,
  now: number,
): PlanWait | null {
  if (plan === null) {
    return null;
  }
  const state = planWaitState(plan);
  if (state === "waiting") {
    return {
      state,
      label: LABEL[state],
      title: TITLE[state],
      detail: `${plan.plan} waits for ${plan.awaited.join(", ")} until ${now >= plan.deadline_at ? "the next check" : formatAsOfMinute(now, plan.deadline_at)}`,
    };
  }
  const others = plan.proposals.filter((proposal) => proposal !== self);
  const together = others.length === 0 ? "" : ` with ${others.join(", ")}`;
  return {
    state,
    label: LABEL[state],
    title: TITLE[state],
    detail: `${plan.plan} waits for the review lead to start one reviewer${together}`,
  };
}
