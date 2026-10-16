import { projectPath, withQuery } from "@/routes/paths";

/** The diff viewer; without `from` it compares against the base recorded for `to`. */
export function diffPath(
  slug: string,
  diff: { to: string; from?: string; path?: string },
): string {
  const params = new URLSearchParams();
  if (diff.from !== undefined) {
    params.set("from", diff.from);
  }
  params.set("to", diff.to);
  if (diff.path !== undefined) {
    params.set("path", diff.path);
  }
  return withQuery(projectPath(slug, "diff"), params);
}

export function proposalPath(slug: string, proposal: string): string {
  return projectPath(slug, "proposals", proposal);
}

export function threadPath(slug: string, thread: string): string {
  return withQuery(projectPath(slug, "messages"), { thread });
}

export function revisionPath(slug: string, revision: string): string {
  return withQuery(projectPath(slug, "repository"), { revision });
}
