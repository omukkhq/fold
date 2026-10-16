import { api } from "./client";
import { unwrap } from "./result";
import type { components } from "./schema";

export type ProjectGit = components["schemas"]["ProjectGit"];
export type GitRemote = components["schemas"]["GitRemote"];

/** The project's Git repository, its branch and each remote with its last push. Rejects with `project_not_found` or `project_unavailable`. */
export async function getProjectGit(slug: string): Promise<ProjectGit> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/git", { params: { path: { slug } } }),
  );
}
