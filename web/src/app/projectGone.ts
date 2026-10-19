import type { QueryClient } from "@tanstack/react-query";
import { ApiError, isApiError } from "../api/errors";
import { keys } from "../api/keys";

/** True for the refusal of a project that fold does not know: a slug that was never registered, or a project fold removed because its `.fold` folder was deleted. */
export function isProjectGone(error: unknown): boolean {
  return isApiError(error, "project_not_found");
}

/** Why fold cannot show a project, by the error code and, for `project_unavailable`, its `details.reason`. */
export type ProjectProblem =
  | { kind: "gone" }
  | {
      kind: "folder_missing" | "database_missing" | "unavailable";
      hint: string | null;
    };

/** The problem of a project screen's error, or null for an error of the screen itself. */
export function projectProblem(error: unknown): ProjectProblem | null {
  if (isProjectGone(error)) {
    return { kind: "gone" };
  }
  if (!(error instanceof ApiError) || error.code !== "project_unavailable") {
    return null;
  }
  const hint =
    typeof error.details.hint === "string" ? error.details.hint : null;
  const reason = error.details.reason;
  return reason === "folder_missing" || reason === "database_missing"
    ? { kind: reason, hint }
    : { kind: "unavailable", hint };
}

/**
 * Drops everything cached for a project that fold removed and marks the project list stale, so the project route loads again, gets `project_not_found` and shows the not-found state, and the list no longer shows the project.
 * `reload` re-runs the route loaders.
 */
export function forgetProject(
  queryClient: QueryClient,
  slug: string,
  reload: () => void,
): void {
  queryClient.removeQueries({ queryKey: [slug] });
  void queryClient.invalidateQueries({ queryKey: keys.projects() });
  reload();
}
