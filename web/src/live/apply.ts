import type { QueryClient, QueryKey } from "@tanstack/react-query";
import { keys } from "../api/keys";
import {
  isRecord,
  liveWorkspaceOf,
  type EventEnvelope,
  type LiveWorkspace,
  type StoredEvent,
} from "./envelope";

const DOMAINS = new Set([
  "agent",
  "worker",
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
  "message",
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
}

/** The part of the board cache the patch rules read: columns by `status`, each with its cards. */
interface BoardShape {
  columns: { status: string; cards: Record<string, unknown>[] }[];
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
  return {
    ...board,
    columns: board.columns.map((column) => {
      const cards = column.cards.filter((c) => c.task !== task);
      return column === target
        ? { ...column, cards: [...cards, patched] }
        : { ...column, cards };
    }),
  };
}

function patchRecord(value: unknown, patch: CardPatch): unknown {
  return isRecord(value) ? { ...value, ...patch } : value;
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
  queryClient.setQueryData(keys.task(slug, event.task), (old: unknown) =>
    patchRecord(old, patch),
  );
}

function applySnapshot(
  queryClient: QueryClient,
  slug: string,
  workspaces: LiveWorkspace[],
): void {
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
  }
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

  switch (domain) {
    case "agent":
    case "worker":
    case "session":
      return [keys.agents(slug)];
    case "task":
    case "label":
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
    case "checkpoint":
    case "changeset":
      return [keys.workspaces(slug), ...workspaceKey, ...taskKey];
    case "proposal":
      return type === "proposal.integrated"
        ? [keys.proposals(slug), ...proposalKey, ...taskKey, repository, board]
        : [keys.proposals(slug), ...proposalKey, ...taskKey, board];
    case "review":
      return [keys.proposals(slug), ...proposalKey, board];
    case "integration":
      return [
        ...(proposal === null ? [] : [keys.proposals(slug), ...proposalKey]),
        ...taskKey,
        repository,
        board,
      ];
    case "main":
      return [repository, board];
    case "remote":
      return [repository];
    default:
      return [keys.messages(slug), board];
  }
}

/** Refetches every observed query of the project after stored events went missing. Live workspace state comes only from the stream, so it is left for the next snapshot. */
export function invalidateProject(
  queryClient: QueryClient,
  slug: string,
): void {
  void queryClient.invalidateQueries({
    queryKey: [slug],
    predicate: (query) => query.queryKey[1] !== keys.workspaceLiveAll(slug)[1],
  });
}

/** A malformed patch falls through to the invalidation of its domain. */
function applyStored(
  queryClient: QueryClient,
  slug: string,
  domain: string,
  event: StoredEvent,
): void {
  if (event.type === "task.status_changed") {
    const status = event.data.status;
    if (typeof status === "string") {
      patchTask(queryClient, slug, event, { status });
      return;
    }
  }
  if (event.type === "task.blocked_changed") {
    const blocked = event.data.blocked;
    if (typeof blocked === "boolean") {
      patchTask(queryClient, slug, event, { blocked });
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
  }
  for (const queryKey of invalidationKeys(slug, event.type, domain, event)) {
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
    queryClient.setQueryData(
      keys.workspaceLive(slug, event.workspace),
      liveWorkspaceOf(event),
    );
    return;
  }
  const domain = domainOf(event.type);
  if (domain !== null) {
    applyStored(queryClient, slug, domain, event);
  }
  void queryClient.invalidateQueries({ queryKey: keys.activity(slug) });
}
