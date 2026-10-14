import { ApiError } from "./errors";
import { fromFixture } from "./fixture";

export interface ProjectSummary {
  slug: string;
  name: string;
}

const fixture = import.meta.env.DEV
  ? () => import("../../fixtures/projects")
  : undefined;

export async function listProjects(): Promise<ProjectSummary[]> {
  return fromFixture(fixture, (m) => m.projects, []);
}

export async function getProject(slug: string): Promise<ProjectSummary> {
  const found = (await listProjects()).find((p) => p.slug === slug);
  if (!found) {
    throw new ApiError("not_found", "project not found");
  }
  return found;
}
