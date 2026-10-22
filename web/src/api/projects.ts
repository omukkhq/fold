import { api } from "./client";
import { unwrap } from "./result";
import { addDays, costDays } from "../format/days";
import type { components } from "./schema";

export type ProjectSummary = components["schemas"]["ProjectSummary"];
export type ProjectOverview = components["schemas"]["ProjectOverview"];
export type PauseEntry = components["schemas"]["PauseEntry"];
export type ProjectFolderState = components["schemas"]["ProjectFolderState"];

/** Every project, the ones a person removed too: the projects page searches them, and every other screen leaves them out. Each project's cost of today counts from `today`, the viewer's own midnight, and the cost of the last seven days from six days before it. */
export async function listProjects(today: number): Promise<ProjectSummary[]> {
  const week = addDays(today, -6);
  const { projects } = await unwrap(
    api.GET("/api/v1/projects", {
      params: { query: { include_removed: true, today, week } },
    }),
  );
  return projects;
}

/** A named human takes the project off fold; its data stays in its folder. The answer's cost of today and of the last seven days starts at the viewer's own midnight, as in the list. Rejects with `project_workers_running` while a worker of the project runs. */
export function removeProject(slug: string): Promise<ProjectSummary> {
  return unwrap(
    api.DELETE("/api/v1/projects/{slug}", {
      params: { path: { slug }, query: costDays(Date.now()) },
    }),
  );
}

/** A named human brings back a removed project whose folder still holds its `.fold/`. Rejects with `project_unavailable` when the folder does not hold it. */
export function restoreProject(slug: string): Promise<ProjectSummary> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/restore", {
      params: { path: { slug }, query: costDays(Date.now()) },
    }),
  );
}

/** Rejects with the daemon's codes: `project_not_found` for an unregistered slug, `project_unavailable` for a project that failed to open. */
export async function getProject(slug: string): Promise<ProjectSummary> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}", {
      params: { path: { slug }, query: costDays(Date.now()) },
    }),
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
