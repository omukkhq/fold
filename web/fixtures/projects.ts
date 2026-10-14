import type { ProjectSummary } from "../src/api/projects";

export const projects: ProjectSummary[] = [
  { slug: "demo", name: "demo", available: true },
  { slug: "ledger", name: "ledger", available: true },
  { slug: "archive", name: "archive", available: false },
];
