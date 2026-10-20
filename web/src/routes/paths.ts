import type { IdKind } from "@/format/ids";

/** `/p/<slug>` followed by the segments, each one encoded as a single path segment. */
export function projectPath(slug: string, ...segments: string[]): string {
  return `/p/${[slug, ...segments].map(encodeURIComponent).join("/")}`;
}

/** The direct chat with one agent. Other screens link here to answer an agent outside any task, so the path stays the same. */
export function agentChatPath(slug: string, agent: string): string {
  return projectPath(slug, "agents", agent, "chat");
}

/** The path with its query string, or the path alone when the query is empty. */
export function withQuery(
  path: string,
  query: URLSearchParams | Record<string, string>,
): string {
  const text = new URLSearchParams(query).toString();
  return text === "" ? path : `${path}?${text}`;
}

/** The screen of a public id; a workspace and a checkpoint open as the diff against them. */
export function idHref(slug: string, kind: IdKind, id: string): string {
  switch (kind) {
    case "task":
      return projectPath(slug, "t", id);
    case "proposal":
      return projectPath(slug, "proposals", id);
    case "request":
      return projectPath(slug, "requests", id);
    case "revision":
      return withQuery(projectPath(slug, "repository"), { revision: id });
    case "message":
      return withQuery(projectPath(slug, "messages"), { thread: id });
    case "changeset":
    case "workspace":
    case "checkpoint":
      return withQuery(projectPath(slug, "diff"), { to: id });
  }
}
