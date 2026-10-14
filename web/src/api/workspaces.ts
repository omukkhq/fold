import type { LiveWorkspace } from "../live/envelope";
import { fromFixture } from "./fixture";

/** No workspace-list route exists yet, so the schema has no type for it; these are the one local declaration of the list's shape. */
export type WorkspaceState = "active" | "missing";

export type AttentionKind =
  "question" | "human_review" | "stalled" | "stale_claim";

/** The daemon decides attention; the web only shows it. */
export interface AttentionItem {
  kind: AttentionKind;
}

export interface ChangesetSummary {
  /** @example C-40 */
  id: string;
  message: string;
  /** UTC milliseconds. */
  recorded_at: number;
  /** The revision the changeset was recorded against, the `from` of its comparison. */
  base: string;
}

export interface WorkspaceEntry {
  /** @example ws-9 */
  workspace: string;
  /** @example T-12 */
  task: string;
  task_title: string;
  /** The worker label, `agent` or `agent/worker`. */
  worker: string;
  state: WorkspaceState;
  /** The revision the workspace is based on. */
  base: string;
  /** The current main revision. */
  main: string;
  /** How many revisions main is ahead of `base`; 0 when the base is current. */
  behind: number;
  /** UTC milliseconds. */
  last_activity_at: number;
  attention: AttentionItem[];
  /** True after a `workspace.watch_degraded` event: changes refresh on a periodic scan. */
  watch_degraded: boolean;
  changesets: ChangesetSummary[];
  /** The live state when the list was read; null for a workspace that is not active. */
  live: LiveWorkspace | null;
}

const fixture = import.meta.env.DEV
  ? () => import("../../fixtures/workspaces")
  : undefined;

export async function listWorkspaces(slug: string): Promise<WorkspaceEntry[]> {
  return fromFixture(fixture, (m) => (slug === "demo" ? m.workspaces : []), []);
}
