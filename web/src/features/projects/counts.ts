import type { ProjectOverview, ProjectSummary } from "../../api/projects";
import { formatSince } from "../../format/time";

/** `17 tasks · 4 in progress · 1 in review · 3 ready · main r8`; zero columns are left out, and a project with every task Done says so. */
export function countsLine(overview: ProjectOverview): string {
  const { backlog, todo, in_progress, review, done } = overview.counts;
  const total = backlog + todo + in_progress + review + done;
  const parts = [`${String(total)} ${total === 1 ? "task" : "tasks"}`];
  if (total > 0 && done === total) {
    parts.push("all Done");
  } else {
    const columns: [number, string][] = [
      [in_progress, "in progress"],
      [review, "in review"],
      [todo, "ready"],
      [backlog, "in backlog"],
    ];
    for (const [count, label] of columns) {
      if (count > 0) {
        parts.push(`${String(count)} ${label}`);
      }
    }
  }
  if (overview.head !== null) {
    parts.push(`main ${overview.head}`);
  }
  return parts.join(" · ");
}

/** `changed 12 s ago`, or `no changes yet`. */
export function lastChangeLine(overview: ProjectOverview, now: number): string {
  return overview.last_change_at === null
    ? "no changes yet"
    : `changed ${formatSince(now, overview.last_change_at)}`;
}

/** The line under a project's name: its counts, or why the daemon has none to give. */
export function summaryLine(project: ProjectSummary, now: number): string {
  if (project.removed_at !== null) {
    return `Removed from fold ${formatSince(now, project.removed_at)}. Its data stays in its folder.`;
  }
  const overview = project.overview;
  if (overview) {
    return countsLine(overview);
  }
  return project.available
    ? "Could not read this project."
    : "Could not open this project.";
}
