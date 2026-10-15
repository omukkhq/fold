import type { ProjectOverview } from "../src/api/projects";

const now = Date.now();

export const projects: ProjectOverview[] = [
  {
    slug: "demo",
    name: "demo",
    available: true,
    counts: { backlog: 5, todo: 3, progress: 4, review: 1, done: 4 },
    agents: ["frontend", "backend", "review"],
    mainRevision: 5,
    lastChangeAt: now - 12_000,
  },
  {
    slug: "ledger",
    name: "ledger",
    available: true,
    counts: { backlog: 0, todo: 0, progress: 0, review: 0, done: 0 },
    agents: [],
    mainRevision: 1,
    lastChangeAt: null,
  },
  {
    slug: "archive",
    name: "archive",
    available: false,
    counts: { backlog: 0, todo: 0, progress: 0, review: 0, done: 0 },
    agents: [],
    mainRevision: null,
    lastChangeAt: null,
  },
];
