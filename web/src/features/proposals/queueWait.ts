import type { components } from "@/api/schema";
import { formatAge } from "@/format/time";

export type IntegrationQueuePlace =
  components["schemas"]["IntegrationQueuePlace"];
export type IntegrationWait = components["schemas"]["IntegrationWait"];

/** The two fields of an approved open proposal that say where its integration stands. */
export interface QueueFields {
  queue: IntegrationQueuePlace | null;
  waits_for: IntegrationWait | null;
}

/**
 * `queue`: the proposal has a place in the automatic integration queue; `person`: it waits for a human; `waiting`: it
 * waits for something else, such as its author or a lane.
 */
export type QueueWaitState = "queue" | "person" | "waiting";

export interface QueueWait {
  state: QueueWaitState;
  /** The short state, such as `2nd in line`. */
  title: string;
  /** What follows the title, such as `after P-12 · starts in about 10 min`; empty when the title says it all. */
  detail: string;
  /** The title and the detail in one line, for a list row or a card. */
  text: string;
}

/** `1st`, `2nd`, `3rd`, `4th`, `11th`, `22nd`. */
export function ordinal(n: number): string {
  const tens = n % 100;
  if (tens >= 11 && tens <= 13) {
    return `${String(n)}th`;
  }
  switch (n % 10) {
    case 1:
      return `${String(n)}st`;
    case 2:
      return `${String(n)}nd`;
    case 3:
      return `${String(n)}rd`;
    default:
      return `${String(n)}th`;
  }
}

/** `starts in about 10 min`, or `starts any moment` once the estimate has passed; empty without an estimate. */
function estimateText(estimate: number | null, now: number): string {
  if (estimate === null) {
    return "";
  }
  return estimate > now
    ? `starts in about ${formatAge(estimate, now)}`
    : "starts any moment";
}

function join(parts: readonly string[]): string {
  return parts.filter((part) => part !== "").join(" · ");
}

function result(
  state: QueueWaitState,
  title: string,
  detail: string,
): QueueWait {
  return {
    state,
    title,
    detail,
    text: detail === "" ? title : `${title} · ${detail}`,
  };
}

function queueText(place: IntegrationQueuePlace, now: number): QueueWait {
  const batch = place.batch === null ? "" : `with batch ${place.batch}`;
  if (place.integrating_now) {
    return result(
      "queue",
      "Integrating now",
      join(["the integration checks run", batch]),
    );
  }
  const estimate = estimateText(place.estimated_start, now);
  if (place.position <= 1) {
    return result("queue", "Next in line", join([batch, estimate]));
  }
  const after = place.ahead === null ? "" : `after ${place.ahead}`;
  return result(
    "queue",
    `${ordinal(place.position)} in line`,
    join([after, batch, estimate]),
  );
}

function waitText(wait: IntegrationWait): QueueWait {
  switch (wait.reason) {
    case "automatic_off":
      return result(
        "waiting",
        "Automatic integration is off",
        "a human integrates it",
      );
    case "config_invalid":
      return result(
        "waiting",
        "Project config is invalid",
        "automatic integration waits until it is valid",
      );
    case "main_checks_invalid":
      return result(
        "waiting",
        "The checks file of main is invalid",
        "automatic integration waits until it is valid",
      );
    case "conflict":
      return result(
        "waiting",
        "Conflict with main",
        "it waits for a new version",
      );
    case "checks_failed":
      return result(
        "waiting",
        "Integration checks failed",
        "it waits for a new version",
      );
    case "lane_refresh":
      return result(
        "waiting",
        `Lane ${wait.lane} asks for a refresh`,
        "its reviewer redoes it on the new main",
      );
    case "lane":
      return result(
        "waiting",
        `Waits in lane ${wait.lane}`,
        `position ${String(wait.position)}`,
      );
    case "batch":
      return result(
        "waiting",
        `Waits for batch ${wait.batch}`,
        "the batch integrates together",
      );
    case "fold_failed_checks":
      return result(
        "waiting",
        "Checks did not finish",
        "integration waits until they run again",
      );
    case "next_version_run":
      return result(
        "waiting",
        "Checks of the next version run",
        "integration waits for them",
      );
    case "prerequisites": {
      const tasks = wait.prerequisites.map((p) => p.task).join(", ");
      return result(
        "waiting",
        "Waits for prerequisites",
        tasks === "" ? "" : `not done: ${tasks}`,
      );
    }
    case "human_review":
      return result(
        "person",
        "Waits for human review",
        wait.reviewer === null ? "any human" : `by ${wait.reviewer}`,
      );
    case "not_approved":
      return result("waiting", "Not approved", "it waits for a review");
  }
}

/**
 * Where an approved open proposal stands in the automatic integration: its place in the queue, or the reason the
 * integration does not take it. A proposal with neither, such as one that is not approved yet, gives null. The place
 * wins when both are set. `now` turns the estimated start into a span.
 */
export function queueWait(
  fields: Partial<QueueFields>,
  now: number,
): QueueWait | null {
  if (fields.queue != null) {
    return queueText(fields.queue, now);
  }
  if (fields.waits_for != null) {
    return waitText(fields.waits_for);
  }
  return null;
}
