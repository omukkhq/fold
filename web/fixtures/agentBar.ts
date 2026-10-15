import type { ProjectAgent } from "../src/api/agentBar";

export function agents(slug: string): ProjectAgent[] {
  if (slug !== "demo") {
    return [];
  }
  return [
    { name: "frontend", presence: "active", working: 2, stalled: 0 },
    { name: "backend", presence: "active", working: 1, stalled: 1 },
    { name: "review", presence: "idle", working: 0, stalled: 0 },
  ];
}
