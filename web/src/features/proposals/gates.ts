import type { Gate, GateName, ProposalDetail } from "@/api/proposals";
import { approvers, displayName } from "./present";

/** The order the integration procedure checks them in. */
const ORDER: readonly GateName[] = [
  "open",
  "version",
  "caller",
  "approval",
  "human_review",
  "prerequisites",
  "merge",
];

/** The gates in check order. A met open or version gate says nothing the header does not, so it is left out. */
export function shownGates(gates: readonly Gate[]): Gate[] {
  return [...gates]
    .filter(
      (g) => (g.gate !== "open" && g.gate !== "version") || g.status !== "met",
    )
    .sort((a, b) => ORDER.indexOf(a.gate) - ORDER.indexOf(b.gate));
}

function names(list: readonly string[]): string {
  const shown = list.map(displayName);
  if (shown.length <= 1) return shown.join("");
  return `${shown.slice(0, -1).join(", ")} and ${shown.at(-1) ?? ""}`;
}

function unfinishedPrerequisites(p: ProposalDetail): string[] {
  return p.prerequisites.filter((t) => t.status !== "done").map((t) => t.task);
}

/**
 * The sentence for one gate. The state is the one the daemon sent for the signed-in human; the words add the facts the payload carries and never test whether the gate holds.
 * `main` is the head of main when the screen knows it.
 */
export function gateText(
  gate: Gate,
  p: ProposalDetail,
  main: string | null,
): string {
  const v = `v${String(p.summary.current_version)}`;
  const met = gate.status === "met";
  switch (gate.gate) {
    case "open":
      return met ? "The proposal is open" : "The proposal is not open";
    case "version":
      return met
        ? `${v} is the current version`
        : `${v} is not the version on screen`;
    case "caller":
      if (met) return "You can integrate as a named human";
      return gate.code === "author_cannot_integrate"
        ? "Your agent wrote this proposal, so you cannot integrate it"
        : "You cannot integrate this proposal";
    case "approval": {
      const by = approvers(p.summary.current_version, p.reviews);
      if (met && by.length > 0) return `Approved on ${v} by ${names(by)}`;
      const changes =
        gate.code === "changes_requested" ||
        p.summary.latest_verdict === "request_changes";
      const head = changes
        ? `Changes requested on ${v}`
        : `No approval on ${v} yet`;
      return met
        ? `${head}; integrating here approves it`
        : `${head}; it needs an approval`;
    }
    case "human_review": {
      const mark = p.summary.human_review;
      if (mark === null) return "No human review asked for";
      const who = mark.reviewer ?? "any human";
      const reason = mark.reason.trim() ? `: "${mark.reason.trim()}"` : "";
      const head = p.summary.human_approved
        ? `Approved by ${who}`
        : `Human review by ${who}`;
      return `${head} · marked by ${displayName(mark.set_by)}${reason}`;
    }
    case "prerequisites": {
      const total = p.prerequisites.length;
      if (total === 0) return "Prerequisites Done: none needed";
      const open = unfinishedPrerequisites(p);
      if (met || open.length === 0) {
        return met
          ? `Prerequisites Done: all ${String(total)}`
          : "Prerequisites not Done";
      }
      return `Prerequisites not Done: ${open.join(", ")}`;
    }
    case "merge": {
      const conflict = p.conflict;
      if (conflict !== null) {
        const n = conflict.files.length;
        const files = `${String(n)} ${n === 1 ? "file conflicts" : "files conflict"}`;
        return `Does not merge onto ${conflict.main_revision}: ${files}`;
      }
      if (met) return "Merges cleanly";
      if (gate.status === "unmet") return "Does not merge onto main";
      return main
        ? `Merge onto ${main} is checked at integration`
        : "The merge is checked at integration";
    }
  }
}

function integrators(p: ProposalDetail): string {
  const holder = p.review_claim?.holder ?? p.summary.review_claim;
  return holder ? `${holder} integrates` : "an integrator integrates";
}

/**
 * The line under the gates that says what the proposal waits on, or null when nothing is left to wait for.
 * It reads the gate states, the turn, the latest verdict and the human approval the daemon sent; it decides nothing.
 */
export function waitingSummary(p: ProposalDetail): string | null {
  const s = p.summary;
  if (s.state === "integrated") return null;
  if (s.state === "closed") {
    return s.close_reason
      ? `Closed without integration: ${s.close_reason}`
      : "Closed without integration.";
  }
  const v = `v${String(s.current_version)}`;
  const author = displayName(s.author);
  if (s.conflicted) {
    return `fold sent the report to ${author}. The task is back in In Progress until a new version arrives.`;
  }
  if (s.turn === "with_author") {
    return `Waiting for ${author} to send a new version.`;
  }
  if (s.human_review && !s.human_approved) {
    const who = s.human_review.reviewer ?? "a human";
    return `Waiting for ${who}. When ${who} approves ${v}, ${integrators(p)}, or ${who} can integrate here.`;
  }
  if (s.latest_verdict !== "approve") {
    return `Waiting for a review of ${v}.`;
  }
  const unmet = shownGates(p.gates).find((g) => g.status === "unmet");
  if (unmet?.gate === "prerequisites") {
    const open = unfinishedPrerequisites(p);
    return open.length > 0
      ? `Waiting for ${open.join(", ")} to be Done.`
      : "Waiting for the prerequisites to be Done.";
  }
  if (unmet) return "Waiting on the gates above.";
  return `Every gate is met. Next, ${integrators(p)}, or a human can integrate here.`;
}
