import { apiErrorCode } from "@/api/errors";
import type { ProposalDetail } from "@/api/proposals";

export type HumanAction = "approve" | "request_changes" | "integrate";

/**
 * The actions the signed-in human sees on the current version. A human with no name can do nothing; otherwise each action shows only when the payload allows it.
 * The daemon has already weighed the gates: Integrate shows whenever it allows it, also after a `request_changes`, because an integrating human approves in the same action.
 */
export function visibleActions(
  proposal: ProposalDetail,
  human: string | null,
): HumanAction[] {
  if (human === null || proposal.state !== "open") return [];
  const allowed = proposal.human_actions;
  const out: HumanAction[] = [];
  if (allowed.approve) out.push("approve");
  if (allowed.request_changes) out.push("request_changes");
  if (allowed.integrate) out.push("integrate");
  return out;
}

/** The composer opens only on the current version of an open proposal, for a named human the payload lets comment. */
export function canComment(
  proposal: ProposalDetail,
  human: string | null,
  shownVersion: number | null,
): boolean {
  return (
    human !== null &&
    proposal.state === "open" &&
    proposal.human_actions.comment &&
    shownVersion === proposal.current_version
  );
}

const TEXTS: Record<string, string> = {
  version_superseded:
    "A newer version arrived. Read it before you send a verdict.",
  proposal_not_open: "The proposal is no longer open.",
  proposal_not_found: "fold has no proposal with this id.",
  gate_unmet: "A gate is not met, so fold did not integrate.",
  human_name_required: "Tell fold your name first, then try once more.",
  auth_required: "Sign in again, then try once more.",
};

/** The sentence for a failed write whose code the screen knows; null for any other failure, which the screen shows with its message. */
export function actionErrorText(error: unknown): string | null {
  const code = apiErrorCode(error);
  return code === null ? null : (TEXTS[code] ?? null);
}
