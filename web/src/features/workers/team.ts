import type { AgentView } from "@/api/agents";
import type { BoardCard, BoardLabel } from "@/api/board";
import type { ReviewBatch } from "@/api/reviews";
import type {
  AreaEntry,
  RoleEntry,
  WorkerList,
  WorkerProcess,
  WorkerQuestion,
  WorkerRole,
  WorkerState,
} from "@/api/workers";
import type { WorkerSteps } from "@/live/apply";
import { type SessionView, sessionView } from "./session";

/** The look of a state pill; each tone has its soft fill, dot and text colour. */
export type PillTone =
  | "working"
  | "stopping"
  | "idle"
  | "failed"
  | "starting"
  | "stopped"
  | "ready"
  | "paused";

export interface PillSpec {
  fill: string;
  dot: string;
  text: string;
}

const PILLS: Record<PillTone, PillSpec> = {
  working: {
    fill: "bg-primary-soft",
    dot: "bg-primary",
    text: "text-primary-strong",
  },
  stopping: {
    fill: "bg-warning-soft",
    dot: "bg-warning",
    text: "text-warning",
  },
  idle: { fill: "bg-sunken", dot: "bg-presence-idle", text: "text-text-muted" },
  failed: { fill: "bg-danger-soft", dot: "bg-danger", text: "text-danger" },
  starting: {
    fill: "bg-sunken",
    dot: "bg-text-faint",
    text: "text-text-muted",
  },
  stopped: {
    fill: "bg-sunken",
    dot: "bg-presence-offline",
    text: "text-text-faint",
  },
  ready: { fill: "bg-success-soft", dot: "bg-success", text: "text-success" },
  paused: { fill: "bg-warning-soft", dot: "bg-warning", text: "text-text" },
};

export function pillSpec(tone: PillTone): PillSpec {
  return PILLS[tone];
}

export interface StatePill {
  label: string;
  tone: PillTone;
}

/** A worker's process state is its pill: the word is the state's own, the tone its colour. */
export function workerPill(state: WorkerState): StatePill {
  return { label: state, tone: state };
}

/** While the project's agents are paused, a worker that was working, idle or starting has no process and runs again in its own session at the resume. */
export function isPausedWorker(state: WorkerState, paused: boolean): boolean {
  return (
    paused && (state === "working" || state === "idle" || state === "starting")
  );
}

const PAUSED_PILL: StatePill = { label: "paused", tone: "paused" };

/** The main agent has no process fold runs; its pill is the agent's presence. */
export function mainPill(presence: AgentView["presence"]): StatePill {
  switch (presence) {
    case "active":
      return { label: "active", tone: "working" };
    case "idle":
      return { label: "idle", tone: "idle" };
    default:
      return { label: "offline", tone: "stopped" };
  }
}

const ROLE_NAMES: Record<WorkerRole, string> = {
  area_lead: "Area lead",
  review_lead: "Review lead",
  task_worker: "Task worker",
  reviewer: "Reviewer",
};

export function roleName(role: WorkerRole): string {
  return ROLE_NAMES[role];
}

/** The agent of an actor such as `web-lead/sub`: its first segment. */
export function actorAgent(actor: string): string {
  const slash = actor.indexOf("/");
  return slash === -1 ? actor : actor.slice(0, slash);
}

/** An area's task label with its colour; a label the board does not know yet is drawn in stone. */
export function areaLabel(
  areas: readonly AreaEntry[],
  labels: readonly BoardLabel[],
  name: string,
): BoardLabel | null {
  const area = areas.find((a) => a.name === name);
  if (!area) return null;
  return (
    labels.find((l) => l.name === area.label) ?? {
      name: area.label,
      color: "stone",
    }
  );
}

function plural(count: number, noun: string): string {
  return `${String(count)} ${noun}${count === 1 ? "" : "s"}`;
}

/** What a row is bound to: a person, a count, or ids that link to a batch. */
export type Bound =
  | { kind: "person"; name: string }
  | { kind: "text"; text: string }
  | { kind: "ids"; text: string; batch: string | null };

/** What the row's process is doing: `tool` is the tool of a step, shown first and stronger. */
export interface Step {
  tool: string | null;
  text: string;
  /** A sentence about the process rather than a step it reported. */
  faint: boolean;
}

export type StopControl = "stop" | "stopping" | null;

/** The last error of a worker as its row shows it under the step: Claude Code's text, and what to check when the daemon says the worker cannot reach the model. */
export interface RowError {
  title: string;
  /** Claude Code's text, shown as data. */
  text: string;
  /** The daemon's hint, with its commands between backticks; null when the worker still reaches the model. */
  hint: string | null;
}

/** The row's error line; null for a worker whose last turn ended without an error. */
export function rowError(worker: WorkerProcess): RowError | null {
  if (worker.last_error === null) return null;
  return {
    title: worker.model_unreachable ? "cannot reach the model" : "last error",
    text: worker.last_error,
    hint: worker.hint,
  };
}

export interface TeamRow {
  name: string;
  /** A child row is a short-lived worker, drawn under its lead. */
  child: boolean;
  role: string;
  /** The line under the worker's name, as the row draws it. */
  roleLine: string;
  pill: StatePill;
  /** The worker waits for a human to answer a permission question. */
  asks: boolean;
  bound: Bound;
  model: string;
  turns: string;
  cost: string;
  context: string;
  step: Step;
  error: RowError | null;
  stop: StopControl;
  /** A named human may start the worker again in a new session. */
  restart: boolean;
  /** The ids of the sessions the worker had before its current one, oldest first. */
  previousSessions: string[];
  /** The worker's Claude Code session; null for the main agent, whose session fold does not run. */
  session: SessionView | null;
}

export interface TeamGroup {
  key: string;
  kind: "main" | "area" | "review";
  /** The area's task label with its colour. */
  label: BoardLabel | null;
  rows: TeamRow[];
  /** The area lead's open task count, for an area group. */
  owned: number;
  area: string | null;
  /** The area's profile, for an area group; empty for every other group. */
  profile: AreaProfileView;
}

/** What an area's profile sets, as its group head shows it: short facts on the head line, the instructions after them. */
export interface AreaProfileView {
  facts: string[];
  instructions: string | null;
}

const NO_PROFILE: AreaProfileView = { facts: [], instructions: null };

/** `model opus` and `MCP fold, pencil` for the fields the profile sets; a field it leaves to the role is not shown. */
export function areaProfile(area: AreaEntry | null): AreaProfileView {
  if (area === null) return NO_PROFILE;
  const facts = [
    ...(area.model === null ? [] : [`model ${area.model}`]),
    ...(area.mcp === null ? [] : [`MCP ${area.mcp.join(", ")}`]),
  ];
  return { facts, instructions: area.instructions };
}

export interface TeamInput {
  workers: WorkerList;
  agents: readonly AgentView[];
  questions: readonly WorkerQuestion[];
  steps: WorkerSteps;
  batches: readonly ReviewBatch[];
  cards: readonly BoardCard[];
  labels: readonly BoardLabel[];
  /** The signed-in human, who runs the main agent in a terminal. */
  human: string | null;
}

const NONE = "-";

/** `$5.87`; the daemon counts millionths of a US dollar. */
export function costText(micro: number): string {
  return `$${(micro / 1_000_000).toFixed(2)}`;
}

/** `164k`; a worker without a request yet has no context. */
export function contextText(tokens: number): string {
  if (tokens === 0) return NONE;
  if (tokens < 1000) return String(tokens);
  return `${String(Math.round(tokens / 1000))}k`;
}

const STEP_TEXTS: Record<string, string> = {
  init: "session started",
  thinking: "thinking",
  text: "writing",
  tool_result: "reading a tool result",
  permission_request: "waits for your answer",
  permission_denied: "was denied",
  result: "turn finished",
  result_error: "turn failed",
};

/** A step of a working process as the live frames report it: the tool and its kind, never its input. */
export function stepOf(step: WorkerSteps[string]): Step {
  if (step.step === "tool_use") {
    return {
      tool: step.tool,
      text: step.tool === null ? "calls a tool" : "",
      faint: false,
    };
  }
  return {
    tool: step.step.startsWith("permission_") ? step.tool : null,
    text: STEP_TEXTS[step.step] ?? step.step,
    faint: false,
  };
}

function faint(text: string): Step {
  return { tool: null, text, faint: true };
}

/** The live step while the process works; otherwise a sentence about the process. */
export function workerStep(
  worker: WorkerProcess,
  steps: WorkerSteps,
  waitsForTaskWorkers = false,
): Step {
  const live = steps[worker.name];
  switch (worker.state) {
    case "working":
      return live ? stepOf(live) : faint("in a turn");
    case "starting":
      return faint("starting the process");
    case "stopping":
      return faint("waiting for the process to exit");
    case "idle":
      return faint(
        waitsForTaskWorkers
          ? "waiting for task workers"
          : "waiting for its next turn",
      );
    default:
      return faint(
        worker.exit_code === null
          ? worker.state
          : `exit code ${String(worker.exit_code)}`,
      );
  }
}

/** A worker that fold is not stopping can start again in a new session; a running one is stopped first. */
export function canRestart(state: WorkerState): boolean {
  return state !== "stopping";
}

/** The role, then how many sessions the worker had before its current one. */
export function roleLine(worker: WorkerProcess): string {
  const earlier = worker.previous_sessions.length;
  if (earlier === 0) return roleName(worker.role);
  const sessions =
    earlier === 1 ? "1 earlier session" : `${String(earlier)} earlier sessions`;
  return `${roleName(worker.role)} · ${sessions}`;
}

export function stopControl(state: WorkerState): StopControl {
  if (state === "stopping") return "stopping";
  return state === "stopped" || state === "failed" ? null : "stop";
}

/** The workspace an agent's worker holds on a task, from the agents answer. */
export function workspaceOf(
  agents: readonly AgentView[],
  name: string,
  task: string,
): string | null {
  for (const agent of agents) {
    for (const worker of agent.workers) {
      if (worker.name !== name) continue;
      const found = worker.workspaces.find((ws) => ws.task === task);
      if (found) return found.ws;
    }
  }
  return null;
}

/** The batch a reviewer holds: its open one, else the newest it held. */
export function heldBatch(
  batches: readonly ReviewBatch[],
  holder: string,
): ReviewBatch | null {
  const held = batches.filter((b) => b.holder === holder);
  return held.find((b) => b.state === "open") ?? held.at(-1) ?? null;
}

/** The proposals of the open batches, each once. */
export function batchedProposals(batches: readonly ReviewBatch[]): string[] {
  const ids = batches
    .filter((b) => b.state === "open")
    .flatMap((b) => b.members.filter((m) => m.in_batch).map((m) => m.proposal));
  return [...new Set(ids)];
}

/** Open tasks of an area's label: what its lead owns. */
export function ownedTasks(cards: readonly BoardCard[], label: string): number {
  return cards.filter(
    (card) =>
      card.status !== "done" && card.labels.some((l) => l.name === label),
  ).length;
}

function boundOf(
  worker: WorkerProcess,
  input: TeamInput,
  areas: readonly AreaEntry[],
): Bound {
  switch (worker.role) {
    case "area_lead": {
      const area = areas.find((a) => a.name === worker.area);
      return area
        ? {
            kind: "text",
            text: plural(ownedTasks(input.cards, area.label), "task"),
          }
        : { kind: "text", text: NONE };
    }
    case "review_lead": {
      const count = batchedProposals(input.batches).length;
      return {
        kind: "text",
        text: count === 0 ? NONE : plural(count, "proposal"),
      };
    }
    case "task_worker": {
      if (worker.tasks.length === 0) return { kind: "text", text: NONE };
      const text = worker.tasks
        .map((task) => {
          const ws = workspaceOf(input.agents, worker.name, task);
          return ws === null ? task : `${task} · ${ws}`;
        })
        .join(", ");
      return { kind: "ids", text, batch: null };
    }
    default: {
      const batch = heldBatch(input.batches, worker.name);
      const proposals =
        worker.proposals.length > 0
          ? worker.proposals
          : (batch?.members.filter((m) => m.in_batch).map((m) => m.proposal) ??
            []);
      const parts = [
        ...(batch ? [batch.id] : []),
        ...(proposals.length > 0 ? [proposals.join(", ")] : []),
      ];
      return parts.length === 0
        ? { kind: "text", text: NONE }
        : { kind: "ids", text: parts.join(" · "), batch: batch?.id ?? null };
    }
  }
}

function workerRow(
  worker: WorkerProcess,
  input: TeamInput,
  child: boolean,
): TeamRow {
  if (isPausedWorker(worker.state, input.workers.pause.paused_since !== null)) {
    return {
      ...runningRow(worker, input, child),
      pill: PAUSED_PILL,
      step: faint("paused · resumes in its own session"),
      stop: null,
      restart: false,
    };
  }
  return runningRow(worker, input, child);
}

function runningRow(
  worker: WorkerProcess,
  input: TeamInput,
  child: boolean,
): TeamRow {
  return {
    name: worker.name,
    child,
    role: roleName(worker.role),
    roleLine: roleLine(worker),
    pill: workerPill(worker.state),
    asks: input.questions.some(
      (q) => q.worker === worker.name && q.answer === null,
    ),
    bound: boundOf(worker, input, input.workers.areas),
    model: worker.model ?? roleModel(input.workers.roles, worker.role) ?? NONE,
    turns: String(worker.turns),
    cost: costText(worker.cost_micro_usd),
    context: contextText(worker.context_tokens),
    step: workerStep(
      worker,
      input.steps,
      waitsForTaskWorkers(worker, input.workers.workers),
    ),
    error: rowError(worker),
    stop: stopControl(worker.state),
    restart: canRestart(worker.state),
    previousSessions: worker.previous_sessions.map((s) => s.session_id),
    session: sessionView(worker),
  };
}

/** The main agent's step when a connected session cannot get fold's notices, so it learns of a message only when it reads its inbox; null otherwise. */
export function noticeStep(agent: AgentView): Step | null {
  if (agent.sessions === 0) return null;
  switch (agent.notices) {
    case "unreachable":
      return faint("not reachable · reads new messages from its inbox");
    case "failing":
      return faint("notices fail · reads new messages from its inbox");
    default:
      return null;
  }
}

function mainRow(agent: AgentView, human: string | null): TeamRow {
  return {
    name: agent.name,
    child: false,
    role: "Main agent",
    roleLine: "Main agent · your terminal session",
    pill: mainPill(agent.presence),
    asks: false,
    bound:
      human === null
        ? { kind: "text", text: NONE }
        : { kind: "person", name: `human:${human}` },
    model: NONE,
    turns: NONE,
    cost: NONE,
    context: NONE,
    step: noticeStep(agent) ?? faint(NONE),
    error: null,
    stop: null,
    restart: false,
    previousSessions: [],
    session: null,
  };
}

/** The model `[roles.<role>] model` sets; null when the role sets none or the daemon could not read the config. */
export function roleModel(
  roles: readonly RoleEntry[],
  role: WorkerRole,
): string | null {
  return roles.find((entry) => entry.role === role)?.model ?? null;
}

const LEADS: readonly WorkerRole[] = ["area_lead", "review_lead"];

/** A lead between turns waits for the task workers or reviewers it started while one of them starts or works. */
export function waitsForTaskWorkers(
  lead: WorkerProcess,
  workers: readonly WorkerProcess[],
): boolean {
  return (
    LEADS.includes(lead.role) &&
    workers.some(
      (w) =>
        !LEADS.includes(w.role) &&
        actorAgent(w.started_by) === lead.name &&
        (w.state === "working" || w.state === "starting"),
    )
  );
}

/**
 * The team: the main agents first, then one group per area with its lead and the task workers under it, then the review lead with its reviewers.
 * A main agent is an agent fold does not supervise: the user's own Claude Code session.
 */
export function teamGroups(input: TeamInput): TeamGroup[] {
  const { workers, areas } = input.workers;
  const supervised = new Set(workers.map((w) => w.name));
  const groups: TeamGroup[] = [];
  const mains = input.agents.filter((agent) => !supervised.has(agent.name));
  if (mains.length > 0) {
    groups.push({
      key: "main",
      kind: "main",
      label: null,
      area: null,
      owned: 0,
      profile: NO_PROFILE,
      rows: mains.map((agent) => mainRow(agent, input.human)),
    });
  }
  const areaNames = [
    ...new Set([
      ...areas.map((a) => a.name),
      ...workers
        .filter((w) => w.role === "area_lead" || w.role === "task_worker")
        .map((w) => w.area ?? ""),
    ]),
  ];
  for (const name of areaNames) {
    const area = areas.find((a) => a.name === name) ?? null;
    const members = workers.filter(
      (w) =>
        (w.role === "area_lead" || w.role === "task_worker") &&
        (w.area ?? "") === name,
    );
    groups.push({
      key: `area:${name}`,
      kind: "area",
      label: areaLabel(areas, input.labels, name),
      area: name === "" ? null : name,
      owned: area ? ownedTasks(input.cards, area.label) : 0,
      profile: areaProfile(area),
      rows: orderRows(members, input),
    });
  }
  const review = workers.filter(
    (w) => w.role === "review_lead" || w.role === "reviewer",
  );
  if (review.length > 0) {
    groups.push({
      key: "review",
      kind: "review",
      label: null,
      area: null,
      owned: 0,
      profile: NO_PROFILE,
      rows: orderRows(review, input),
    });
  }
  return groups;
}

/** Leads first in start order; each short-lived worker follows the lead that started it, and the rest follow the last lead. */
function orderRows(
  members: readonly WorkerProcess[],
  input: TeamInput,
): TeamRow[] {
  const leads = members.filter((w) => LEADS.includes(w.role));
  const children = members.filter((w) => !LEADS.includes(w.role));
  const placed = new Set<string>();
  const rows: TeamRow[] = [];
  leads.forEach((lead, index) => {
    rows.push(workerRow(lead, input, false));
    const last = index === leads.length - 1;
    for (const child of children) {
      if (placed.has(child.name)) continue;
      if (actorAgent(child.started_by) === lead.name || last) {
        placed.add(child.name);
        rows.push(workerRow(child, input, true));
      }
    }
  });
  if (leads.length === 0) {
    rows.push(...children.map((child) => workerRow(child, input, true)));
  }
  return rows;
}

/** `area · lead owns 4 tasks`; an area named apart from its label says its name. */
export function areaNote(group: TeamGroup): string {
  const name =
    group.area === null || group.area === group.label?.name
      ? "area"
      : `area ${group.area}`;
  return group.label === null
    ? name
    : `${name} · lead owns ${plural(group.owned, "task")}`;
}

/** The team has no row at all: no agent connected and fold runs no worker. */
export function isEmptyTeam(groups: readonly TeamGroup[]): boolean {
  return groups.every((group) => group.rows.length === 0);
}

/** Only main agents so far: no worker runs or ran, so the leads are still to start. */
export function hasNoLeads(groups: readonly TeamGroup[]): boolean {
  return groups.every(
    (group) => group.kind === "main" || group.rows.length === 0,
  );
}

/** The main agent's name for the no-leads hint; `main` before the session connects. */
export function mainAgentName(groups: readonly TeamGroup[]): string {
  return groups.find((g) => g.kind === "main")?.rows[0]?.name ?? "main";
}

/** The row of the worker that asks the open question `id`. */
export function askingRow(
  groups: readonly TeamGroup[],
  questions: readonly WorkerQuestion[],
  id: string,
): TeamRow | null {
  const worker = questions.find((q) => q.id === id)?.worker;
  return (
    groups.flatMap((g) => g.rows).find((row) => row.name === worker) ?? null
  );
}
