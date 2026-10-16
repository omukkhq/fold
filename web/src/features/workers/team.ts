import type { AgentView } from "@/api/agents";
import type { BoardCard, BoardLabel } from "@/api/board";
import type { ReviewBatch } from "@/api/reviews";
import type {
  AreaEntry,
  WorkerList,
  WorkerProcess,
  WorkerQuestion,
  WorkerRole,
  WorkerState,
} from "@/api/workers";
import type { WorkerSteps } from "@/live/apply";

/** The look of a state pill; each tone has its soft fill, dot and text colour. */
export type PillTone =
  | "working"
  | "stopping"
  | "idle"
  | "failed"
  | "starting"
  | "stopped"
  | "ready";

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
  task_coder: "Task coder",
  batch_reviewer: "Batch reviewer",
};

export function roleName(role: WorkerRole): string {
  return ROLE_NAMES[role];
}

/** `Task coder · cache 5m`; the cache time is left out while fold cannot read the config. */
export function roleLine(
  worker: Pick<WorkerProcess, "role" | "cache_ttl">,
): string {
  const name = roleName(worker.role);
  return worker.cache_ttl === null
    ? name
    : `${name} · cache ${worker.cache_ttl}`;
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

export interface TeamRow {
  name: string;
  /** A child row is a short-lived worker, drawn under its lead. */
  child: boolean;
  role: string;
  /** The role with its cache time, as the row draws it. */
  roleLine: string;
  pill: StatePill;
  /** The worker waits for a human to answer a permission question. */
  asks: boolean;
  bound: Bound;
  model: string;
  turns: string;
  cost: string;
  cacheHits: string;
  context: string;
  step: Step;
  stop: StopControl;
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

/** The share of cached prompt tokens that were read from the cache rather than written to it. */
export function cacheHitsText(read: number, write: number): string {
  const total = read + write;
  return total === 0 ? NONE : `${String(Math.round((read * 100) / total))}%`;
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
export function workerStep(worker: WorkerProcess, steps: WorkerSteps): Step {
  const live = steps[worker.name];
  switch (worker.state) {
    case "working":
      return live ? stepOf(live) : faint("in a turn");
    case "starting":
      return faint("starting the process");
    case "stopping":
      return faint("waiting for the process to exit");
    case "idle":
      return faint("waiting for its next turn");
    default:
      return faint(
        worker.exit_code === null
          ? worker.state
          : `exit code ${String(worker.exit_code)}`,
      );
  }
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

/** The batch a batch reviewer holds: its open one, else the newest it held. */
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
    case "task_coder": {
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
    model: worker.model ?? NONE,
    turns: String(worker.turns),
    cost: costText(worker.cost_micro_usd),
    cacheHits: cacheHitsText(
      worker.cache_read_tokens,
      worker.cache_write_tokens,
    ),
    context: contextText(worker.context_tokens),
    step: workerStep(worker, input.steps),
    stop: stopControl(worker.state),
  };
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
    cacheHits: NONE,
    context: NONE,
    step: faint(NONE),
    stop: null,
  };
}

const LEADS: readonly WorkerRole[] = ["area_lead", "review_lead"];

/**
 * The team as W09 draws it: the main agents first, then one group per area with its lead and the task coders under it, then the review lead with its batch reviewers.
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
      rows: mains.map((agent) => mainRow(agent, input.human)),
    });
  }
  const areaNames = [
    ...new Set([
      ...areas.map((a) => a.name),
      ...workers
        .filter((w) => w.role === "area_lead" || w.role === "task_coder")
        .map((w) => w.area ?? ""),
    ]),
  ];
  for (const name of areaNames) {
    const area = areas.find((a) => a.name === name) ?? null;
    const members = workers.filter(
      (w) =>
        (w.role === "area_lead" || w.role === "task_coder") &&
        (w.area ?? "") === name,
    );
    groups.push({
      key: `area:${name}`,
      kind: "area",
      label: areaLabel(areas, input.labels, name),
      area: name === "" ? null : name,
      owned: area ? ownedTasks(input.cards, area.label) : 0,
      rows: orderRows(members, input),
    });
  }
  const review = workers.filter(
    (w) => w.role === "review_lead" || w.role === "batch_reviewer",
  );
  if (review.length > 0) {
    groups.push({
      key: "review",
      kind: "review",
      label: null,
      area: null,
      owned: 0,
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
