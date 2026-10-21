import type { ProjectSummary } from "../api/projects";

/** The switcher lists only the projects a person is likely to want next; the projects page has the rest. */
export const RECENT_PROJECTS = 5;

export interface SwitcherEntry {
  slug: string;
  name: string;
  /** The slug, shown beside a name that another listed project shares; null for a name of its own. */
  detail: string | null;
}

/** The most recently used projects in fold, newest first by the daemon's `last_used_at` and then by slug, at most `RECENT_PROJECTS`; a removed project is left out. */
export function recentProjects(
  projects: readonly ProjectSummary[],
): SwitcherEntry[] {
  const recent = projects
    .filter((project) => project.removed_at === null)
    .sort(
      (a, b) => b.last_used_at - a.last_used_at || a.slug.localeCompare(b.slug),
    )
    .slice(0, RECENT_PROJECTS);
  const counts = new Map<string, number>();
  for (const project of recent) {
    counts.set(project.name, (counts.get(project.name) ?? 0) + 1);
  }
  return recent.map((project) => ({
    slug: project.slug,
    name: project.name,
    detail: (counts.get(project.name) ?? 0) > 1 ? project.slug : null,
  }));
}
