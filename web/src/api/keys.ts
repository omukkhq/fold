/**
 * Every key of project data starts with the slug, so one prefix invalidates or cancels a whole project.
 * Host keys start with "@", which no slug can, so a project named "projects" never shares their prefix.
 */
export const keys = {
  projects: () => ["@projects"] as const,
  session: () => ["@session"] as const,
  project: (slug: string) => [slug, "project"] as const,
  board: (slug: string) => [slug, "board"] as const,
  task: (slug: string, task: number) => [slug, "task", task] as const,
  activity: (slug: string) => [slug, "activity"] as const,
  proposals: (slug: string) => [slug, "proposals"] as const,
  proposal: (slug: string, proposal: number) =>
    [slug, "proposal", proposal] as const,
  agents: (slug: string) => [slug, "agents"] as const,
  messages: (slug: string) => [slug, "messages"] as const,
  repository: (slug: string) => [slug, "repository"] as const,
};
