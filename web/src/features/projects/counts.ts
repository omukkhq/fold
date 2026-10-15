import type { ProjectOverview } from "../../api/projects";
import { formatSince } from "../../format/time";

/** `17 tasks · 4 in progress · 1 in review · 3 ready · main r8`; zero columns are left out, and a project with every task Done says so. */
export function countsLine(project: ProjectOverview): string {
  const { backlog, todo, progress, review, done } = project.counts;
  const total = backlog + todo + progress + review + done;
  const parts = [`${String(total)} ${total === 1 ? "task" : "tasks"}`];
  if (total > 0 && done === total) {
    parts.push("all Done");
  } else {
    const columns: [number, string][] = [
      [progress, "in progress"],
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
  if (project.mainRevision !== null) {
    parts.push(`main r${String(project.mainRevision)}`);
  }
  return parts.join(" · ");
}

/** `changed 12 s ago`, or `no changes yet`. */
export function lastChangeLine(project: ProjectOverview, now: number): string {
  return project.lastChangeAt === null
    ? "no changes yet"
    : `changed ${formatSince(now, project.lastChangeAt)}`;
}
