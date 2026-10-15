import type { components } from "../api/schema";

export type EventEnvelope = components["schemas"]["EventEnvelope"];
export type LiveWorkspace = components["schemas"]["LiveWorkspace"];
export type Snapshot = components["schemas"]["Snapshot"];
export type StoredEvent = Extract<EventEnvelope, { kind: "event" }>;
export type WorkspaceChanged = Extract<
  EventEnvelope,
  { kind: "workspace_changed" }
>;
export type SnapshotFrame = Extract<EventEnvelope, { kind: "snapshot" }>;
export type PresenceChanged = Extract<
  EventEnvelope,
  { kind: "agent_presence_changed" }
>;

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isSeq(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 0;
}

function isCount(value: unknown): boolean {
  return typeof value === "number" && Number.isInteger(value) && value >= 0;
}

function isNullableString(value: unknown): boolean {
  return value === null || typeof value === "string";
}

const CHANGE_KINDS: readonly unknown[] = [
  "added",
  "modified",
  "deleted",
  "renamed",
] satisfies components["schemas"]["ChangeKind"][];

const PRESENCE_STATES: readonly unknown[] = [
  "active",
  "idle",
  "offline",
] satisfies components["schemas"]["PresenceState"][];

function isChangedFile(value: unknown): boolean {
  return (
    isRecord(value) &&
    typeof value.path === "string" &&
    isNullableString(value.previous_path) &&
    CHANGE_KINDS.includes(value.change) &&
    isCount(value.added_lines) &&
    isCount(value.removed_lines) &&
    typeof value.binary === "boolean"
  );
}

function isCheckpoint(value: unknown): boolean {
  return (
    isRecord(value) &&
    typeof value.id === "string" &&
    isNullableString(value.label) &&
    typeof value.automatic === "boolean" &&
    typeof value.created_at === "number"
  );
}

export function isLiveWorkspace(value: unknown): value is LiveWorkspace {
  return (
    isRecord(value) &&
    typeof value.workspace === "string" &&
    typeof value.task === "string" &&
    typeof value.worker === "string" &&
    typeof value.base === "string" &&
    Array.isArray(value.files) &&
    value.files.every(isChangedFile) &&
    typeof value.summary === "string" &&
    typeof value.last_activity_at === "number" &&
    Array.isArray(value.checkpoints) &&
    value.checkpoints.every(isCheckpoint) &&
    isNullableString(value.proposal) &&
    typeof value.stalled === "boolean"
  );
}

/** A `type` this build does not know passes, because the daemon can be newer than the web build: its `seq` still moves the cursor, and `apply.ts` refreshes the activity list and, when the domain is known, that domain's keys. */
function isStoredEvent(value: Record<string, unknown>): boolean {
  return (
    isSeq(value.seq) &&
    typeof value.type === "string" &&
    value.type !== "" &&
    typeof value.actor === "string" &&
    isNullableString(value.task) &&
    isNullableString(value.proposal) &&
    isNullableString(value.workspace) &&
    isNullableString(value.revision) &&
    isRecord(value.data) &&
    typeof value.created_at === "number"
  );
}

/** Checks a parsed frame against the generated `EventEnvelope`: a `kind` this build does not know is refused. */
export function isEventEnvelope(value: unknown): value is EventEnvelope {
  if (!isRecord(value)) {
    return false;
  }
  switch (value.kind) {
    case "event":
      return isStoredEvent(value);
    case "workspace_changed":
      return isLiveWorkspace(value);
    case "agent_presence_changed":
      return (
        typeof value.agent === "string" &&
        PRESENCE_STATES.includes(value.presence)
      );
    case "snapshot":
      return (
        isSeq(value.seq) &&
        Array.isArray(value.workspaces) &&
        value.workspaces.every(isLiveWorkspace)
      );
    default:
      return false;
  }
}

export function isSnapshot(frame: EventEnvelope): frame is SnapshotFrame {
  return frame.kind === "snapshot";
}

/** The live state of a `workspace_changed` frame without its tag, the same value a snapshot entry holds. */
export function liveWorkspaceOf(frame: WorkspaceChanged): LiveWorkspace {
  return {
    workspace: frame.workspace,
    task: frame.task,
    worker: frame.worker,
    base: frame.base,
    files: frame.files,
    summary: frame.summary,
    last_activity_at: frame.last_activity_at,
    checkpoints: frame.checkpoints,
    proposal: frame.proposal,
    stalled: frame.stalled,
  };
}
