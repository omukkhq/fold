import { api } from "./client";
import { unwrap } from "./result";
import type { components } from "./schema";

export type ProjectSummary = components["schemas"]["ProjectSummary"];
export type ProjectOverview = components["schemas"]["ProjectOverview"];
export type PauseEntry = components["schemas"]["PauseEntry"];

export async function listProjects(): Promise<ProjectSummary[]> {
  const { projects } = await unwrap(api.GET("/api/v1/projects"));
  return projects;
}

/** Rejects with the daemon's codes: `project_not_found` for an unregistered slug, `project_unavailable` for a project that failed to open. */
export async function getProject(slug: string): Promise<ProjectSummary> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}", { params: { path: { slug } } }),
  );
}

/** A named human pauses every agent of the project; it answers with the pause state. Pausing a paused project changes nothing. */
export function pauseProject(slug: string): Promise<PauseEntry> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/pause", { params: { path: { slug } } }),
  );
}

/** A named human ends the pause; it answers with the pause state. Resuming a project that is not paused changes nothing. */
export function resumeProject(slug: string): Promise<PauseEntry> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/resume", { params: { path: { slug } } }),
  );
}
