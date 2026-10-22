import type { QueryClient, QueryKey } from "@tanstack/react-query";
import type { DurableEvent } from "../api/events";
import { keys } from "../api/keys";
import { withEvent } from "./eventList";
import {
  isRecord,
  liveWorkspaceOf,
  type EventEnvelope,
  type LiveWorkspace,
  type PresenceChanged,
  type StoredEvent,
  type WorkerOutput,
} from "./envelope";

const DOMAINS = new Set([
  "agent",
  "area",
  "worker",
  "workers",
  "session",
  "task",
  "label",
  "dependency",
  "checklist",
  "claim",
  "workspace",
  "checkpoint",
  "changeset",
  "proposal",
  "review",
  "integration",
  "main",
  "remote",
  "git",
  "message",
  "request",
  "project",
  "human_change",
  "stash",
  "checks",
]);

export function domainOf(type: string): string | null {
  const dot = type.indexOf(".");
  if (dot <= 0) {
    return null;
  }
  const domain = type.slice(0, dot);
  return DOMAINS.has(domain) ? domain : null;
}

interface CardPatch {
  status?: string;
  blocked?: boolean;
  /** The unfinished prerequisites at the moment the flag changed. */
  blocked_by?: string[];
}

/** The part of the board cache the patch rules read: columns by `status`, each with its cards and the count of every task of the status. */
interface BoardShape {
  columns: {
    status: string;
    cards: Record<string, unknown>[];
    total?: number;
  }[];
}

function isBoard(value: unknown): value is BoardShape {
  return (
    isRecord(value) &&
    Array.isArray(value.columns) &&
    value.columns.every(
      (column) =>
        isRecord(column) &&
        typeof column.status === "string" &&
        Array.isArray(column.cards) &&
        column.cards.every(isRecord),
    )
  );
}

/** Returns the board with the card patched and moved to the column of its new status, or null when the cache cannot place it. */
export function patchBoard(
  board: unknown,
  task: string,
  patch: CardPatch,
): BoardShape | null {
  if (!isBoard(board)) {
    return null;
  }
  const source = board.columns.find((column) =>
    column.cards.some((card) => card.task === task),
  );
  const card = source?.cards.find((c) => c.task === task);
  if (!source || !card) {
    return null;
  }
  const target =
    patch.status === undefined
      ? source
      : board.columns.find((column) => column.status === patch.status);
  if (!target) {
    return null;
  }
  const patched = { ...card, ...patch };
  const moved = target !== source;
  return {
    ...board,
    columns: board.columns.map((column) => {
      const cards = column.cards.filter((c) => c.task !== task);
      const next =
        column === target
          ? { ...column, cards: [...cards, patched] }
          : { ...column, cards };
      if (moved && typeof column.total === "number") {
        if (column === source) {
          return { ...next, total: column.total - 1 };
        }
        if (column === target) {
          return { ...next, total: column.total + 1 };
        }
      }
      return next;
    }),
  };
}

function isTaskIds(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((v) => typeof v === "string");
}

function patchRecord(value: unknown, patch: CardPatch): unknown {
  return isRecord(value) ? { ...value, ...patch } : value;
}

/** Each record of `list` that names `task` takes the new status; null when none does. */
function withStatus(
  list: unknown,
  task: string,
  status: string,
): unknown[] | null {
  if (!Array.isArray(list)) {
    return null;
  }
  const records: unknown[] = list;
  const names = (v: unknown) => isRecord(v) && v.task === task;
  if (!records.some(names)) {
    return null;
  }
  return records.map((v) => (isRecord(v) && names(v) ? { ...v, status } : v));
}

/** Another task read names the task in its dependency edges and linked checklist items, each with the task's status. */
export function patchNamedStatus(
  read: unknown,
  task: string,
  status: string,
): unknown {
  if (!isRecord(read)) {
    return read;
  }
  const next: Record<string, unknown> = { ...read };
  let changed = false;
  const dependencies = withStatus(read.dependencies, task, status);
  if (dependencies !== null) {
    next.dependencies = dependencies;
    changed = true;
  }
  if (Array.isArray(read.checklists)) {
    next.checklists = read.checklists.map((checklist: unknown) => {
      if (!isRecord(checklist)) {
        return checklist;
      }
      const items = withStatus(checklist.items, task, status);
      if (items === null) {
        return checklist;
      }
      changed = true;
      return { ...checklist, items };
    });
  }
  return changed ? next : read;
}

function patchOtherReads(
  queryClient: QueryClient,
  slug: string,
  task: string,
  status: string,
): void {
  for (const query of queryClient
    .getQueryCache()
    .findAll({ queryKey: [slug, "task"] })) {
    if (query.queryKey[2] === task || query.state.data === undefined) {
      continue;
    }
    const next = patchNamedStatus(query.state.data, task, status);
    if (next !== query.state.data) {
      queryClient.setQueryData(query.queryKey, next);
    }
  }
}

function patchTask(
  queryClient: QueryClient,
  slug: string,
  event: StoredEvent,
  patch: CardPatch,
): void {
  if (event.task === null) {
    return;
  }
  const board = queryClient.getQueryData(keys.board(slug));
  if (board !== undefined) {
    const next = patchBoard(board, event.task, patch);
    if (next) {
      queryClient.setQueryData(keys.board(slug), next);
    } else {
      void queryClient.invalidateQueries({ queryKey: keys.board(slug) });
    }
  }
  // A task read has no `blocked_by`: it lists the edges with their statuses.
  const taskPatch: CardPatch = {};
  if (patch.status !== undefined) taskPatch.status = patch.status;
  if (patch.blocked !== undefined) taskPatch.blocked = patch.blocked;
  queryClient.setQueryData(keys.task(slug, event.task), (old: unknown) =>
    patchRecord(old, taskPatch),
  );
  if (patch.status !== undefined) {
    patchOtherReads(queryClient, slug, event.task, patch.status);
  }
}

const DIFF_KEYS = new Set(["comparison", "file-diff"]);

/** A comparison or file diff with the workspace on either side reads working files, which a change makes stale. */
function invalidateWorkspaceDiffs(
  queryClient: QueryClient,
  slug: string,
  workspace: string,
): void {
  void queryClient.invalidateQueries({
    predicate: ({ queryKey }) =>
      queryKey[0] === slug &&
      typeof queryKey[1] === "string" &&
      DIFF_KEYS.has(queryKey[1]) &&
      (queryKey[2] === workspace || queryKey[3] === workspace),
  });
}

function endedWorkspaces(queryClient: QueryClient, slug: string): string[] {
  const ended = queryClient.getQueryData(keys.endedWorkspaces(slug));
  return Array.isArray(ended)
    ? ended.filter((id): id is string => typeof id === "string")
    : [];
}

/** The watcher can publish a change after the commit that ended the workspace; the snapshot after that knows the workspace is gone. */
function markEnded(
  queryClient: QueryClient,
  slug: string,
  workspace: string,
): void {
  const ended = endedWorkspaces(queryClient, slug);
  if (!ended.includes(workspace)) {
    queryClient.setQueryData(keys.endedWorkspaces(slug), [...ended, workspace]);
  }
}

function applyWorkspaceChanged(
  queryClient: QueryClient,
  slug: string,
  live: LiveWorkspace,
): void {
  if (endedWorkspaces(queryClient, slug).includes(live.workspace)) {
    return;
  }
  queryClient.setQueryData(keys.workspaceLive(slug, live.workspace), live);
  invalidateWorkspaceDiffs(queryClient, slug, live.workspace);
  void queryClient.invalidateQueries({
    queryKey: keys.workspaceTrees(slug, live.workspace),
  });
}

/** The snapshot replaces every live state, so a workspace that ended while the client was away disappears, and the diffs of each live workspace are read again for the changes the client missed. */
function applySnapshot(
  queryClient: QueryClient,
  slug: string,
  workspaces: LiveWorkspace[],
): void {
  queryClient.removeQueries({
    queryKey: keys.endedWorkspaces(slug),
    exact: true,
  });
  const present = new Set(workspaces.map((w) => w.workspace));
  for (const query of queryClient
    .getQueryCache()
    .findAll({ queryKey: keys.workspaceLiveAll(slug) })) {
    const id = query.queryKey[2];
    if (typeof id === "string" && !present.has(id)) {
      queryClient.removeQueries({ queryKey: query.queryKey, exact: true });
    }
  }
  for (const workspace of workspaces) {
    queryClient.setQueryData(
      keys.workspaceLive(slug, workspace.workspace),
      workspace,
    );
    invalidateWorkspaceDiffs(queryClient, slug, workspace.workspace);
    void queryClient.invalidateQueries({
      queryKey: keys.workspaceTrees(slug, workspace.workspace),
    });
  }
}

/** A request event and a request-linked message name the request in `data.request`: request events carry no task or proposal id. */
function requestOf(event: StoredEvent): string | null {
  const request = event.data.request;
  return typeof request === "string" ? request : null;
}

/** The ids of the tasks `request.approved` created. */
function createdTasks(event: StoredEvent): string[] {
  const tasks = event.data.tasks;
  return isTaskIds(tasks) ? tasks : [];
}

/** The version of a proposal that an event names in `data.version`, such as the version a check run checked or a review reviewed. */
function versionKeys(slug: string, event: StoredEvent): QueryKey[] {
  const version = event.data.version;
  return event.proposal !== null &&
    typeof version === "number" &&
    Number.isInteger(version) &&
    version >= 1
    ? [keys.proposalVersion(slug, event.proposal, version)]
    : [];
}

function invalidationKeys(
  slug: string,
  type: string,
  domain: string,
  event: StoredEvent,
): QueryKey[] {
  const { task, proposal, workspace } = event;
  const taskKey = task === null ? [] : [keys.task(slug, task)];
  const proposalKey = proposal === null ? [] : [keys.proposal(slug, proposal)];
  const workspaceKey =
    workspace === null ? [] : [keys.workspace(slug, workspace)];
  const board = keys.board(slug);
  const repository = keys.repository(slug);
  const request = requestOf(event);
  const requestKey = request === null ? [] : [keys.request(slug, request)];

  switch (domain) {
    case "agent":
    case "session":
      return [keys.agents(slug)];
    case "worker":
      return [
        keys.agents(slug),
        keys.workers(slug),
        keys.workerQuestions(slug),
        keys.needsYou(slug),
      ];
    case "area":
    case "workers":
      return [keys.workers(slug)];
    case "label":
      return [board, keys.labels(slug), ...taskKey];
    case "task": {
      // Needs you lists human tasks in Todo and tasks with a blocker, which any task event can add or remove; the human review mark also decides which proposals wait for the human. A change of the labels can move the task to another area, whose lead reads its feedback.
      const readers = task === null ? [] : [keys.taskReaders(slug, task)];
      return type === "task.human_review_set" ||
        type === "task.human_review_cleared"
        ? [
            board,
            ...taskKey,
            ...readers,
            keys.needsYou(slug),
            keys.proposals(slug),
          ]
        : [board, ...taskKey, ...readers, keys.needsYou(slug)];
    }
    case "dependency":
    case "checklist":
      return [board, ...taskKey];
    case "claim":
      return [
        board,
        keys.workspaces(slug),
        keys.agents(slug),
        ...taskKey,
        ...workspaceKey,
      ];
    case "workspace":
      // A card names its task's live workspace, which opens, closes and moves with these events.
      return [keys.workspaces(slug), ...workspaceKey, ...taskKey, board];
    case "checkpoint":
    case "changeset":
      return [keys.workspaces(slug), ...workspaceKey, ...taskKey];
    case "proposal":
      return [
        keys.proposals(slug),
        ...proposalKey,
        ...taskKey,
        board,
        keys.reviewBatches(slug),
        keys.needsYou(slug),
        ...(type === "proposal.integrated" ? [repository] : []),
      ];
    case "review":
      // A review plan changes the waiting state of each of its proposals and of their board cards; its events name only one of them, or none.
      if (type.startsWith("review.plan_")) {
        return [
          keys.proposals(slug),
          keys.proposalPages(slug),
          board,
          ...taskKey,
        ];
      }
      return type.startsWith("review.batch_")
        ? [keys.reviewBatches(slug), keys.workers(slug), ...proposalKey]
        : [
            keys.proposals(slug),
            ...proposalKey,
            ...taskKey,
            board,
            keys.reviewBatches(slug),
            keys.needsYou(slug),
          ];
    case "checks": {
      const run = typeof event.data.run === "string" ? event.data.run : null;
      // A review batch's integration run shows on every member's page, but its events name only the member that stores it.
      const pages =
        event.data.stage === "integration"
          ? [keys.proposalPages(slug)]
          : proposalKey;
      return [
        keys.proposals(slug),
        ...pages,
        ...taskKey,
        board,
        ...(run === null ? [] : [keys.checkRun(slug, run)]),
      ];
    }
    case "integration":
      return [
        ...(proposal === null ? [] : [keys.proposals(slug), ...proposalKey]),
        ...taskKey,
        repository,
        board,
        // A batch's queued integration and each member's integration change what the batch screen says about its integration.
        ...(typeof event.data.batch === "string"
          ? [keys.reviewBatches(slug)]
          : []),
        // Needs you lists main's invalid checks file from the first integration it holds.
        ...(event.type === "integration.held" ? [keys.needsYou(slug)] : []),
      ];
    case "main":
      // Needs you lists main's invalid checks file only while main stays on the revision that holds the integrations. A batch's merged change is built on main's head.
      return [repository, board, keys.needsYou(slug), keys.reviewBatches(slug)];
    case "remote":
      return [repository, keys.git(slug)];
    case "human_change":
      // Needs you lists a conflicted human change until an integration brings it onto main or a newer one replaces it.
      return [
        repository,
        board,
        keys.git(slug),
        keys.project(slug),
        keys.needsYou(slug),
      ];
    case "git":
      // `git.committed` names the revision whose commit the revision, proposal and task reads show; Needs you lists a Git branch that fold did not move and a project folder that waits on the human's changes.
      return type === "git.committed"
        ? [repository, keys.git(slug), keys.proposals(slug), keys.tasks(slug)]
        : type === "git.head_moved" ||
            type === "git.uncommitted_changes" ||
            type === "git.attention_cleared"
          ? [
              repository,
              keys.git(slug),
              keys.project(slug),
              keys.needsYou(slug),
            ]
          : [repository, keys.git(slug), keys.project(slug)];
    case "stash":
      // Needs you lists an unstash whose conflict markers wait in the project folder.
      return [
        repository,
        keys.git(slug),
        keys.project(slug),
        keys.needsYou(slug),
      ];
    case "project":
      return [keys.project(slug), keys.workers(slug), keys.agents(slug)];
    case "request":
      return type === "request.approved"
        ? [
            keys.requests(slug),
            ...requestKey,
            board,
            ...createdTasks(event).map((t) => keys.task(slug, t)),
            keys.needsYou(slug),
          ]
        : [keys.requests(slug), ...requestKey, keys.needsYou(slug)];
    default:
      // A message to a human can open an ask, and a reply or a closure ends one, so the linked task's question attention changes with it.
      return [
        keys.messages(slug),
        board,
        ...taskKey,
        ...requestKey,
        keys.needsYou(slug),
      ];
  }
}

/** Returns the agent list with the named agent's presence replaced, or null when the cache is not a list of named records or does not hold the agent. */
export function patchPresence(
  agents: unknown,
  frame: Pick<PresenceChanged, "agent" | "presence">,
): unknown[] | null {
  if (!Array.isArray(agents) || !agents.every(isRecord)) {
    return null;
  }
  if (!agents.some((agent) => agent.name === frame.agent)) {
    return null;
  }
  return agents.map((agent) =>
    agent.name === frame.agent ? { ...agent, presence: frame.presence } : agent,
  );
}

/** The agent lists under the agents prefix take the new presence in place; a list that cannot take it, such as one that does not hold the agent yet, is refetched. Caches without presence records, such as a task's feedback readers, are left alone. */
function applyPresence(
  queryClient: QueryClient,
  slug: string,
  frame: PresenceChanged,
): void {
  for (const query of queryClient
    .getQueryCache()
    .findAll({ queryKey: keys.agents(slug) })) {
    const data = query.state.data;
    if (!Array.isArray(data)) {
      continue;
    }
    const next = patchPresence(data, frame);
    if (next) {
      queryClient.setQueryData(query.queryKey, next);
    } else {
      void queryClient.invalidateQueries({
        queryKey: query.queryKey,
        exact: true,
      });
    }
  }
}

const STREAM_ONLY = new Set<unknown>([
  keys.workspaceLiveAll("")[1],
  keys.endedWorkspaces("")[1],
  keys.workerSteps("")[1],
  keys.revisionTree("", "", "")[1],
  keys.revisionFile("", "", "")[1],
]);

/** Refetches every observed query of the project after stored events went missing. Live workspace state and worker steps come only from the stream, so they are left for the next frames; a revision's trees and files never change. */
export function invalidateProject(
  queryClient: QueryClient,
  slug: string,
): void {
  void queryClient.invalidateQueries({
    queryKey: [slug],
    predicate: (query) => !STREAM_ONLY.has(query.queryKey[1]),
  });
}

/** The latest step of each worker, by agent name, as the live `worker.output` frames report it. */
export type WorkerSteps = Readonly<
  Record<string, Pick<WorkerOutput, "step" | "tool">>
>;

function applyWorkerOutput(
  queryClient: QueryClient,
  slug: string,
  frame: WorkerOutput,
): void {
  queryClient.setQueryData<WorkerSteps>(keys.workerSteps(slug), (old) => ({
    ...old,
    [frame.agent]: { step: frame.step, tool: frame.tool },
  }));
  // A turn's start and end move the worker between working and idle without a stored event, so the agents and the workers are read again then.
  if (TURN_STEPS.has(frame.step)) {
    void queryClient.invalidateQueries({ queryKey: keys.agents(slug) });
    void queryClient.invalidateQueries({ queryKey: keys.workers(slug) });
  }
  // A turn's end changes the item of a worker that could not reach the model without a stored event: a turn that reaches the model ends it, and a failed one changes its error. The daemon stores the turn's cost before it sends the frame, so the costs are read again too.
  if (frame.step === "result" || frame.step === "result_error") {
    void queryClient.invalidateQueries({ queryKey: keys.needsYou(slug) });
    void queryClient.invalidateQueries({ queryKey: keys.cost(slug) });
    void queryClient.invalidateQueries({ queryKey: keys.projects() });
  }
}

/** The steps that start or end a worker's turn. */
const TURN_STEPS: ReadonlySet<string> = new Set([
  "init",
  "result",
  "result_error",
]);

/** A malformed patch falls through to the invalidation of its domain. */
function applyStored(
  queryClient: QueryClient,
  slug: string,
  domain: string,
  event: StoredEvent,
): void {
  if (event.type === "task.status_changed") {
    const status = event.data.to;
    if (typeof status === "string") {
      patchTask(queryClient, slug, event, { status });
      return;
    }
  }
  if (event.type === "task.blocked_changed") {
    const { blocked, blocked_by: blockedBy } = event.data;
    if (typeof blocked === "boolean" && isTaskIds(blockedBy)) {
      patchTask(queryClient, slug, event, {
        blocked,
        blocked_by: blockedBy,
      });
      return;
    }
  }
  if (
    (event.type === "workspace.abandoned" ||
      event.type === "workspace.archived") &&
    event.workspace !== null
  ) {
    queryClient.removeQueries({
      queryKey: keys.workspaceLive(slug, event.workspace),
      exact: true,
    });
    markEnded(queryClient, slug, event.workspace);
  }
  for (const queryKey of [
    ...invalidationKeys(slug, event.type, domain, event),
    ...versionKeys(slug, event),
  ]) {
    void queryClient.invalidateQueries({ queryKey });
  }
}

/**
 * Patches the cache where the event carries the new state and invalidates the keys of its domain otherwise.
 * Every stored event is a row of the activity list, so the activity key is invalidated even for a type this build does not know. A type this build does not know is handled by its domain, and a type of an unknown domain touches nothing else.
 * An integration of kind `pull` carries no proposal or task id, so it touches no proposal key.
 */
export function applyEvent(
  queryClient: QueryClient,
  slug: string,
  event: EventEnvelope,
): void {
  if (event.kind === "snapshot") {
    applySnapshot(queryClient, slug, event.workspaces);
    return;
  }
  if (event.kind === "workspace_changed") {
    applyWorkspaceChanged(queryClient, slug, liveWorkspaceOf(event));
    return;
  }
  if (event.kind === "agent_presence_changed") {
    applyPresence(queryClient, slug, event);
    return;
  }
  if (event.kind === "worker_output") {
    applyWorkerOutput(queryClient, slug, event);
    return;
  }
  // The thread's delivery lines read `pushed_at`, which the daemon committed before it sent the frame.
  if (event.kind === "message_pushed") {
    void queryClient.invalidateQueries({
      queryKey: keys.messagesThread(slug, event.thread),
    });
    return;
  }
  const durable: DurableEvent = {
    seq: event.seq,
    type: event.type,
    actor: event.actor,
    task: event.task,
    proposal: event.proposal,
    workspace: event.workspace,
    revision: event.revision,
    data: event.data,
    created_at: event.created_at,
  };
  queryClient.setQueryData(keys.events(slug), (old: unknown) =>
    Array.isArray(old) ? withEvent<DurableEvent>(old, durable) : undefined,
  );
  const domain = domainOf(event.type);
  if (domain !== null) {
    applyStored(queryClient, slug, domain, event);
  }
  void queryClient.invalidateQueries({ queryKey: keys.activity(slug) });
}
