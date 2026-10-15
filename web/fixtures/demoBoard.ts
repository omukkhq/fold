import type { BoardCard, TaskStatus } from "../src/api/board";

const SECOND = 1_000;
const MINUTE = 60 * SECOND;

const FRONTEND = { name: "frontend", color: "plum" };
const BACKEND = { name: "backend", color: "slate" };
const UI = { name: "ui", color: "sage" };
const DATA = { name: "data", color: "gold" };
const AUTH = { name: "auth", color: "ink" };
const INFRA = { name: "infra", color: "teal" };

function card(
  task: string,
  title: string,
  status: TaskStatus,
  extra: Partial<BoardCard> = {},
): BoardCard {
  return {
    task,
    title,
    status,
    blocked: false,
    labels: [],
    checklist: {
      steps_done: 0,
      steps_total: 0,
      linked_done: 0,
      linked_total: 0,
    },
    needs: [],
    blocks: 0,
    worker: null,
    workspace: null,
    activity: null,
    proposal: null,
    human_review: null,
    attention: [],
    ...extra,
  };
}

function steps(done: number, total: number, linkedDone = 0, linkedTotal = 0) {
  return {
    steps_done: done,
    steps_total: total,
    linked_done: linkedDone,
    linked_total: linkedTotal,
  };
}

/** The `demo` project's board; tasks a human creates in the dialog come on top of it. */
export function demoCards(now: number): BoardCard[] {
  return [
    card("T-5", "Login flow", "backlog", {
      labels: [FRONTEND, { name: "phase-1", color: "stone" }],
      checklist: steps(1, 2, 0, 1),
      needs: ["T-2", "T-4"],
      blocks: 1,
    }),
    card("T-6", "Wire the dashboard to the reports API", "backlog", {
      labels: [FRONTEND, { name: "wiring", color: "moss" }],
      checklist: steps(0, 4),
      needs: ["T-4"],
      attention: [{ kind: "question" }],
    }),
    card("T-7", "Export a report as CSV", "backlog", {
      labels: [BACKEND, DATA],
      checklist: steps(0, 3),
      needs: ["T-4"],
    }),
    card("T-1", "Add the session cookie and sign-out", "todo", {
      labels: [BACKEND, AUTH],
      checklist: steps(0, 5),
      blocks: 3,
      worker: "frontend/ui",
    }),
    card("T-8", "Empty and error states for the dashboard", "todo", {
      labels: [FRONTEND, UI],
      checklist: steps(0, 4),
      human_review: { reviewer: "tanim" },
    }),
    card("T-9", "Rate-limit the login endpoint", "todo", {
      labels: [BACKEND, AUTH],
      checklist: steps(0, 2),
    }),
    card("T-2", "Build the login form with validation", "in_progress", {
      labels: [FRONTEND, UI],
      checklist: steps(3, 4, 1, 1),
      blocks: 1,
      worker: "frontend/ui",
      workspace: "ws-2",
      activity: { path: "src/routes/login.tsx", at: now - 12 * SECOND },
    }),
    card("T-4", "Reports API: list, filter and paginate", "in_progress", {
      labels: [BACKEND],
      checklist: steps(2, 6),
      blocks: 3,
      worker: "backend/api",
      workspace: "ws-4",
      activity: { path: "reports/filter.rs", at: now - 2 * SECOND },
    }),
    card("T-10", "Seed data for local development", "in_progress", {
      labels: [BACKEND, INFRA],
      blocked: true,
      checklist: steps(1, 3),
      needs: ["T-3"],
      worker: "backend/db",
      workspace: "ws-10",
      activity: { path: "seed/reports.sql", at: now - 6 * MINUTE },
      attention: [{ kind: "stalled" }],
    }),
    card("T-11", "Design tokens and the theme switch", "in_progress", {
      labels: [FRONTEND, UI],
      checklist: steps(5, 5),
      worker: "frontend/styles",
      workspace: "ws-11",
      activity: { path: "src/styles/tokens.css", at: now - 47 * MINUTE },
      proposal: { id: "P-3", state: "changes_requested", revision: null },
      attention: [{ kind: "stale_claim" }],
    }),
    card("T-12", "Chart the weekly totals", "in_progress", {
      labels: [FRONTEND, DATA],
      checklist: steps(2, 3),
      worker: "frontend/charts",
      workspace: "ws-12",
      activity: { path: "src/charts/weekly.tsx", at: now - MINUTE },
      proposal: { id: "P-6", state: "conflict", revision: null },
    }),
    card("T-3", "Database schema for reports", "review", {
      labels: [BACKEND, DATA],
      checklist: steps(4, 4),
      blocks: 2,
      worker: "backend/db",
      proposal: { id: "P-4", state: "approved", revision: null },
      human_review: { reviewer: "tanim" },
      attention: [{ kind: "human_review" }],
    }),
    card("T-13", "Form field components", "done", {
      labels: [FRONTEND, UI],
      proposal: { id: "P-5", state: "integrated", revision: "r8" },
    }),
    card("T-14", "Error envelope and health route", "done", {
      labels: [BACKEND],
      proposal: { id: "P-7", state: "integrated", revision: "r7" },
    }),
    card("T-15", "Scaffold the API service", "done", {
      labels: [BACKEND, INFRA],
      checklist: steps(5, 5),
      proposal: { id: "P-2", state: "integrated", revision: "r6" },
    }),
    card("T-16", "Scaffold the web app", "done", {
      labels: [FRONTEND, INFRA],
      checklist: steps(4, 4),
      proposal: { id: "P-1", state: "integrated", revision: "r5" },
    }),
  ];
}
