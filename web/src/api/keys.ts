/**
 * Task, proposal and workspace keys take the public ids (`T-12`, `P-3`, `ws-9`) that the API and the event stream carry.
 * Every key of project data starts with the slug, so one prefix invalidates or cancels a whole project.
 * Host keys start with "@", which no slug can, so a project named "projects" never shares their prefix.
 */
export const keys = {
  projects: () => ["@projects"] as const,
  session: () => ["@session"] as const,
  project: (slug: string) => [slug, "project"] as const,
  board: (slug: string) => [slug, "board"] as const,
  task: (slug: string, task: string) => [slug, "task", task] as const,
  activity: (slug: string) => [slug, "activity"] as const,
  /** A comparison without `from` is `to` against its recorded base. */
  comparison: (slug: string, from: string | undefined, to: string) =>
    [slug, "comparison", from ?? null, to] as const,
  fileDiff: (
    slug: string,
    from: string | undefined,
    to: string,
    path: string,
  ) => [slug, "file-diff", from ?? null, to, path] as const,
  proposals: (slug: string) => [slug, "proposals"] as const,
  proposal: (slug: string, proposal: string) =>
    [slug, "proposal", proposal] as const,
  agents: (slug: string) => [slug, "agents"] as const,
  /** Under the agents prefix, so every event that invalidates the agents also refreshes the agent chips of the top bar. */
  agentBar: (slug: string) => [slug, "agents", "bar"] as const,
  /** Under the agents prefix, so the feedback box learns a new agent or tag with the agent list. */
  messageTargets: (slug: string) => [slug, "agents", "targets"] as const,
  labels: (slug: string) => [slug, "labels"] as const,
  messages: (slug: string) => [slug, "messages"] as const,
  taskMessages: (slug: string, task: string) =>
    [slug, "messages", "task", task] as const,
  repository: (slug: string) => [slug, "repository"] as const,
  workspaces: (slug: string) => [slug, "workspaces"] as const,
  workspace: (slug: string, workspace: string) =>
    [slug, "workspace", workspace] as const,
  /** Live working state, written only by the event stream. */
  workspaceLive: (slug: string, workspace: string) =>
    [slug, "workspace-live", workspace] as const,
  workspaceLiveAll: (slug: string) => [slug, "workspace-live"] as const,
};
