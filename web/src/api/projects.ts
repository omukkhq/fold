import { api } from "./client";
import { unwrap } from "./result";
import type { components } from "./schema";

export type ProjectSummary = components["schemas"]["ProjectSummary"];
export type ProjectOverview = components["schemas"]["ProjectOverview"];

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
