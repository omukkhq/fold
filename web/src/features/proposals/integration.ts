import type { IntegrationResult, ProposalSummary } from "@/api/proposals";
import type { Revision } from "@/api/repository";

/** The revision an integrated proposal became, read from the main revisions, which name the proposal they integrated. */
export interface IntegrationView {
  /** @example r9 */
  revision: string;
  /** The Git commit of the revision; null until the daemon writes it, and while only the integrate answer names the revision. */
  commit: string | null;
  /** An integration publishes the current version. */
  version: number;
  /** UTC milliseconds. */
  at: number;
  /** Who integrated; null because neither the revisions nor the integrate answer name the integrating actor, so the slot draws nothing. */
  actor: string | null;
}

/**
 * The revision of main that names the proposal; the revisions are newest first, as the route sends them. Until the revisions name it, the answer of the human's own integrate write gives the revision; a queued answer names none.
 * Null while the proposal is not integrated or nothing names its revision yet.
 */
export function integrationOf(
  p: ProposalSummary,
  revisions: readonly Revision[],
  answered?: IntegrationResult,
): IntegrationView | null {
  if (p.state !== "integrated") return null;
  const revision = revisions.find((r) => r.proposal === p.id);
  if (revision) {
    return {
      revision: revision.id,
      commit: revision.commit,
      version: p.current_version,
      at: revision.created_at,
      actor: null,
    };
  }
  if (answered?.proposal === p.id && answered.revision !== null) {
    return {
      revision: answered.revision,
      commit: null,
      version: p.current_version,
      at: p.closed_at ?? p.updated_at,
      actor: null,
    };
  }
  return null;
}

/** The head of main: the newest revision; null for a project the screen has no revisions of. */
export function mainHead(revisions: readonly Revision[]): string | null {
  return revisions[0]?.id ?? null;
}

/** Each integrated proposal's revision by proposal id, for the list. */
export function integratedRevisions(
  revisions: readonly Revision[],
): ReadonlyMap<string, string> {
  const out = new Map<string, string>();
  for (const r of revisions) {
    if (r.proposal !== null && !out.has(r.proposal)) out.set(r.proposal, r.id);
  }
  return out;
}
