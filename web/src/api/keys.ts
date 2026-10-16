/**
 * Task, proposal and workspace keys take the public ids (`T-12`, `P-3`, `ws-9`) that the API and the event stream carry.
 * Every key of project data starts with the slug, so one prefix invalidates or cancels a whole project.
 * Host keys start with "@", which no slug can, so a project named "projects" never shares their prefix.
 */
export const keys = {
  projects: () => ["@projects"] as const,
  session: () => ["@session"] as const,
  humans: () => ["@humans"] as const,
  project: (slug: string) => [slug, "project"] as const,
  board: (slug: string) => [slug, "board"] as const,
  task: (slug: string, task: string) => [slug, "task", task] as const,
  activity: (slug: string) => [slug, "activity"] as const,
  /** A comparison without `from` is a workspace against its base. `to` sits at index 3 of both diff keys, where a workspace change finds the diffs it makes stale. */
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
  /** Under the proposal prefix, so every event and write that refreshes the proposal also refreshes its comments. */
  proposalComments: (slug: string, proposal: string) =>
    [slug, "proposal", proposal, "comments"] as const,
  /** Outside the proposal prefix: a change between two version numbers never changes, so no event refreshes it. */
  proposalChanges: (
    slug: string,
    proposal: string,
    version: number,
    since: number | null,
  ) => [slug, "proposal-diff", proposal, version, since] as const,
  proposalFileDiff: (
    slug: string,
    proposal: string,
    version: number,
    since: number | null,
    path: string,
  ) => [slug, "proposal-diff", proposal, version, since, path] as const,
  requests: (slug: string) => [slug, "requests"] as const,
  request: (slug: string, request: string) =>
    [slug, "request", request] as const,
  /** Under the request prefix, so every event and write that refreshes the request also refreshes its clarification thread. */
  requestThread: (slug: string, request: string, thread: string) =>
    [slug, "request", request, "thread", thread] as const,
  events: (slug: string) => [slug, "events"] as const,
  agents: (slug: string) => [slug, "agents"] as const,
  /** Under the agents prefix, so every event that invalidates the agents also refreshes the agent chips of the top bar. */
  agentBar: (slug: string) => [slug, "agents", "bar"] as const,
  /** Under the agents prefix, so the feedback box learns a new agent or tag with the agent list. */
  messageTargets: (slug: string) => [slug, "agents", "targets"] as const,
  labels: (slug: string) => [slug, "labels"] as const,
  messages: (slug: string) => [slug, "messages"] as const,
  taskMessages: (slug: string, task: string) =>
    [slug, "messages", "task", task] as const,
  /** Under the messages prefix, so every message event that refreshes the threads also refreshes the count. */
  messagesAwaiting: (slug: string) => [slug, "messages", "awaiting"] as const,
  repository: (slug: string) => [slug, "repository"] as const,
  git: (slug: string) => [slug, "git"] as const,
  /** Every task read of the project, under one prefix. */
  tasks: (slug: string) => [slug, "task"] as const,
  workspaces: (slug: string) => [slug, "workspaces"] as const,
  workspace: (slug: string, workspace: string) =>
    [slug, "workspace", workspace] as const,
  /** Live working state, written only by the event stream. */
  workspaceLive: (slug: string, workspace: string) =>
    [slug, "workspace-live", workspace] as const,
  /** Under the workspaces prefix, so every event that refreshes the workspaces also refreshes the review checkouts. */
  reviewCheckouts: (slug: string) =>
    [slug, "workspaces", "review-checkouts"] as const,
  workspaceLiveAll: (slug: string) => [slug, "workspace-live"] as const,
  workers: (slug: string) => [slug, "workers"] as const,
  workerQuestions: (slug: string) => [slug, "worker-questions"] as const,
  /** Each worker's latest step, written only by the event stream. */
  workerSteps: (slug: string) => [slug, "worker-steps"] as const,
  needsYou: (slug: string) => [slug, "needs-you"] as const,
  reviewBatches: (slug: string) => [slug, "review-batches"] as const,
  /** Under the batches prefix, so every event that refreshes the batches also refreshes the open batch. */
  reviewBatch: (slug: string, batch: string) =>
    [slug, "review-batches", batch] as const,
  /** The workspaces a stored event ended since the last snapshot, written only by the event stream. */
  endedWorkspaces: (slug: string) => [slug, "ended-workspaces"] as const,
};
