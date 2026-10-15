import type { WorkspaceEntry } from "../src/api/workspaces";
import type { LiveWorkspace } from "../src/live/envelope";

const MINUTE = 60_000;
const NOW = Date.now();
const ago = (minutes: number): number => NOW - minutes * MINUTE;

function files(
  paths: [string, LiveWorkspace["files"][number]["change"], number, number][],
): LiveWorkspace["files"] {
  return paths.map(([path, change, added, removed]) => ({
    path,
    previous_path: null,
    change,
    added_lines: added,
    removed_lines: removed,
    binary: false,
  }));
}

const loginFiles = files([
  ["src/routes/login.tsx", "modified", 42, 8],
  ["src/forms/login-schema.ts", "added", 21, 0],
  ["src/api/auth.ts", "modified", 4, 2],
  ["src/styles/login.css", "deleted", 0, 12],
]);

const reportsFiles: LiveWorkspace["files"] = [
  ...files([["src/api/reports/filter.ts", "modified", 17, 3]]),
  {
    path: "src/api/reports/page.ts",
    previous_path: "src/api/reports/paging.ts",
    change: "renamed",
    added_lines: 2,
    removed_lines: 2,
    binary: false,
  },
  {
    path: "docs/reports.png",
    previous_path: null,
    change: "added",
    added_lines: 0,
    removed_lines: 0,
    binary: true,
  },
];

export const workspaces: WorkspaceEntry[] = [
  {
    workspace: "ws-1",
    task: "T-1",
    task_title: "Build the login form with validation",
    worker: "frontend/ui",
    state: "active",
    base: "r7",
    main: "r9",
    behind: 2,
    last_activity_at: ago(1),
    attention: [],
    watch_degraded: false,
    changesets: [
      {
        id: "C-18",
        message: "Form fields and validation",
        recorded_at: ago(25),
        base: "r7",
      },
    ],
    live: {
      workspace: "ws-1",
      task: "T-1",
      worker: "frontend/ui",
      base: "r7",
      files: loginFiles,
      summary: "4 files, +67 -22",
      last_activity_at: ago(1),
      checkpoints: [
        {
          id: "ck-31",
          label: "quiet 2 min",
          automatic: true,
          created_at: ago(3),
        },
      ],
      proposal: null,
      stalled: false,
    },
  },
  {
    workspace: "ws-2",
    task: "T-2",
    task_title: "Reports API: list, filter and paginate",
    worker: "backend/api",
    state: "active",
    base: "r9",
    main: "r9",
    behind: 0,
    last_activity_at: ago(8),
    attention: [{ kind: "stalled" }, { kind: "stale_claim" }],
    watch_degraded: true,
    changesets: [
      {
        id: "C-21",
        message: "List endpoint with cursor",
        recorded_at: ago(9),
        base: "r9",
      },
    ],
    live: {
      workspace: "ws-2",
      task: "T-2",
      worker: "backend/api",
      base: "r9",
      files: reportsFiles,
      summary: "3 files, +19 -5",
      last_activity_at: ago(8),
      checkpoints: [
        {
          id: "ck-40",
          label: "before record",
          automatic: false,
          created_at: ago(8),
        },
      ],
      proposal: null,
      stalled: false,
    },
  },
  {
    workspace: "ws-3",
    task: "T-3",
    task_title: "Seed data for local development",
    worker: "backend/db",
    state: "missing",
    base: "r8",
    main: "r9",
    behind: 1,
    last_activity_at: ago(45),
    attention: [],
    watch_degraded: false,
    changesets: [],
    live: null,
  },
];
