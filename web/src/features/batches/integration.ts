import { ApiError, apiErrorCode } from "@/api/errors";
import type { ReviewBatch } from "@/api/reviews";
import { failureText } from "@/format/failure";

export interface IntegrationNote {
  /** Where the batch's integration stands, as one sentence. */
  text: string;
  /** A human can integrate the batch now: every member has its final state, some members integrate together, and no request of the batch waits. */
  canIntegrate: boolean;
}

function list(ids: readonly string[]): string {
  return ids.join(", ");
}

/** What the screen says about the batch's integration. The approved members of a submitted batch integrate together, with one run of the integration checks, once every member has its final state. */
export function integrationNote(batch: ReviewBatch): IntegrationNote {
  const i = batch.integration;
  switch (i.state) {
    case "not_submitted":
      return {
        text:
          batch.state === "closed"
            ? "The batch closed without a review, so each member integrates on its own."
            : "When the review is submitted, fold integrates the approved members together, with one run of the integration checks.",
        canIntegrate: false,
      };
    case "waiting":
      return {
        text: `Waiting for ${list(i.pending)} to get a final verdict. Then fold integrates the approved members together.`,
        canIntegrate: false,
      };
    case "ready":
      if (i.members.length === 0) {
        return {
          text: "No member can integrate with the batch. Each member integrates on its own.",
          canIntegrate: false,
        };
      }
      if (i.queued) {
        return {
          text: `Queued: fold integrates ${list(i.members)} together in its turn.`,
          canIntegrate: false,
        };
      }
      return {
        text: `Ready: ${list(i.members)} integrate together, with one run of the integration checks.`,
        canIntegrate: true,
      };
    case "ended": {
      const published = i.revisions.map(
        (r) => `${r.proposal} as ${r.revision}`,
      );
      if (i.outcome === "together") {
        return {
          text: `Integrated together: ${list(published)}.`,
          canIntegrate: false,
        };
      }
      if (i.outcome === "one_at_a_time") {
        const run = i.run === null ? "" : ` (${i.run})`;
        return {
          text: `The integration checks of the members together failed${run}, so fold integrated them one at a time.`,
          canIntegrate: false,
        };
      }
      return {
        text: "No member integrated with the batch. Each member integrates on its own.",
        canIntegrate: false,
      };
    }
  }
}

const REASONS: Record<string, string> = {
  batch_waiting: "A member has no final verdict yet.",
  batch_integrated: "The batch is integrated already.",
  batch_not_submitted: "The batch review is not submitted yet.",
  integration_queued: "The batch waits in the integration queue already.",
};

/** The text of a refused batch integrate, by the error code and `details.reason`, never by the message. */
export function integrateFailure(error: unknown): string {
  if (apiErrorCode(error) === "invalid_request" && error instanceof ApiError) {
    const { reason } = error.details;
    if (typeof reason === "string" && reason in REASONS) {
      return REASONS[reason] ?? "";
    }
  }
  switch (apiErrorCode(error)) {
    case "head_moved":
      return "main moved during each attempt. Try again.";
    case "checks_file_invalid":
      return "fold-checks.toml on main is not valid. Integrate again once it is fixed.";
    default:
      return failureText(error);
  }
}
