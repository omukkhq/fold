import type { Gate, GateKind, ProposalDetail } from "@/api/proposals";
import { displayName } from "./present";

/** The order the integration procedure checks them in, with the human review beside the approval it adds to. */
const ORDER: readonly GateKind[] = [
  "approval",
  "human_review",
  "integrator",
  "prerequisites",
  "merge",
];

export function orderGates(gates: readonly Gate[]): Gate[] {
  return [...gates].sort(
    (a, b) => ORDER.indexOf(a.kind) - ORDER.indexOf(b.kind),
  );
}

function names(list: readonly string[]): string {
  const shown = list.map(displayName);
  if (shown.length <= 1) return shown.join("");
  return `${shown.slice(0, -1).join(", ")} and ${shown.at(-1) ?? ""}`;
}

/**
 * The sentence for one gate. The words describe the state the daemon sent; they never test whether the gate holds.
 * `clock` formats a check time, so the caller decides how a time reads.
 */
export function gateText(gate: Gate, clock: (at: number) => string): string {
  switch (gate.kind) {
    case "approval": {
      const v = `v${String(gate.version)}`;
      if (gate.state === "met") {
        return `Approved on ${v} by ${names(gate.approvers)}`;
      }
      if (gate.latest_verdict === "request_changes") {
        return `Changes requested on ${v}; it needs an approval`;
      }
      return `No approval on ${v} yet`;
    }
    case "human_review": {
      const who = gate.reviewer ?? "any human";
      const reason = gate.reason.trim() ? `: "${gate.reason.trim()}"` : "";
      const head =
        gate.state === "met" ? `Approved by ${who}` : `Human review by ${who}`;
      return `${head} · marked by ${displayName(gate.marked_by)}${reason}`;
    }
    case "integrator":
      return gate.agents.length > 0
        ? `Caller needs the integrator tag: ${names(gate.agents)} ${gate.agents.length === 1 ? "has" : "have"} it`
        : "Caller needs the integrator tag: no agent has it";
    case "prerequisites":
      if (gate.total === 0) return "Prerequisites Done: none needed";
      if (gate.unfinished.length === 0) {
        return `Prerequisites Done: all ${String(gate.total)}`;
      }
      return `Prerequisites not Done: ${gate.unfinished.join(", ")}`;
    case "merge": {
      const checked =
        gate.checked_at === null
          ? "not checked yet"
          : `checked ${clock(gate.checked_at)}`;
      if (gate.state === "met") {
        return `Merges cleanly onto ${gate.main} (${checked})`;
      }
      const files = `${String(gate.conflicting_files)} ${gate.conflicting_files === 1 ? "file conflicts" : "files conflict"}`;
      return `Does not merge onto ${gate.main}: ${files} (${checked})`;
    }
  }
}

function integrators(p: ProposalDetail): string {
  if (p.review_claim) return `${p.review_claim} integrates`;
  const gate = p.gates.find((g) => g.kind === "integrator");
  const agent = gate?.kind === "integrator" ? gate.agents[0] : undefined;
  return agent ? `the ${agent} agent integrates` : "an integrator integrates";
}

/**
 * The line under the gates that says what the proposal waits on, or null when nothing is left to wait for.
 * It reads the gate states and the proposal's own fields; it decides nothing.
 */
export function waitingSummary(p: ProposalDetail): string | null {
  if (p.state === "integrated") return null;
  if (p.state === "closed") {
    return p.close_reason
      ? `Closed without integration: ${p.close_reason}`
      : "Closed without integration.";
  }
  const v = `v${String(p.current_version)}`;
  const author = displayName(p.author);
  if (p.conflicted) {
    return `fold sent the report to ${author}. The task is back in In Progress until a new version arrives.`;
  }
  const gates = orderGates(p.gates);
  const human = gates.find((g) => g.state === "waiting_human");
  if (human) {
    const who =
      human.kind === "human_review" && human.reviewer
        ? human.reviewer
        : "a human";
    return `Waiting for ${who}. When ${who} approves ${v}, ${integrators(p)}, or ${who} can integrate here.`;
  }
  if (p.with === "author") {
    return `Waiting for ${author} to send a new version.`;
  }
  const unmet = gates.filter((g) => g.state === "unmet");
  const first = unmet[0];
  if (!first) {
    return `Every gate is met. Next, ${integrators(p)}, or a human can integrate here.`;
  }
  switch (first.kind) {
    case "approval":
      return `Waiting for a review of ${v}.`;
    case "prerequisites":
      return `Waiting for ${first.unfinished.join(", ")} to be Done.`;
    case "integrator":
      return "Waiting for a human to give an agent the integrator tag, or to integrate here.";
    default:
      return "Waiting on the gates above.";
  }
}
