import { api } from "./client";
import { unwrap } from "./result";
import type { components } from "./schema";

type Schemas = components["schemas"];

export type RecordResult = Schemas["RecordResult"];
export type AgentChangesRestored = Schemas["AgentChangesRestored"];
export type AgentChangesIgnored = Schemas["AgentChangesIgnored"];

/** A named human records the project folder, a worker's change in it too. Without `confirm` a change that empties a tracked file, adds many empty files or holds a worker's change rejects with `record_needs_confirmation`, whose details name the files and the `tree` that `confirm` takes. */
export function recordAgentChanges(
  slug: string,
  confirm?: string,
): Promise<RecordResult> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/agent-changes/record", {
      params: {
        path: { slug },
        query: confirm === undefined ? {} : { confirm },
      },
    }),
  );
}

/** A named human puts the files workers changed in the project folder into a new stash entry and brings main's version back. Rejects with `agent_change_not_found` when none waits. */
export function restoreAgentChanges(
  slug: string,
): Promise<AgentChangesRestored> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/agent-changes/restore", {
      params: { path: { slug } },
    }),
  );
}

/** A named human lets the files workers changed count as their own unrecorded change. Rejects with `agent_change_not_found` when none waits. */
export function ignoreAgentChanges(slug: string): Promise<AgentChangesIgnored> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/agent-changes/ignore", {
      params: { path: { slug } },
    }),
  );
}
