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
export type WorkerOutput = Extract<EventEnvelope, { kind: "worker_output" }>;
export type MessagePushed = Extract<EventEnvelope, { kind: "message_pushed" }>;

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

function isNullableBoolean(value: unknown): boolean {
  return value === null || typeof value === "boolean";
}

/** The keys of a record that names every value of a generated enum, so a value the schema gains and this list lacks fails the type check instead of refusing every frame that carries it. */
function everyValue<T extends string>(
  values: Record<T, true>,
): readonly unknown[] {
  return Object.keys(values);
}

const CHANGE_KINDS = everyValue<components["schemas"]["ChangeKind"]>({
  added: true,
  modified: true,
  deleted: true,
  renamed: true,
});

const PRESENCE_STATES = everyValue<components["schemas"]["PresenceState"]>({
  active: true,
  idle: true,
  offline: true,
});

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

/** A purpose this build does not know passes, because the daemon can be newer than the web build. */
function isCheckRun(value: unknown): boolean {
  return (
    isRecord(value) &&
    typeof value.run === "string" &&
    typeof value.purpose === "string" &&
    isNullableString(value.proposal) &&
    typeof value.started_at === "number"
  );
}

export function isLiveWorkspace(value: unknown): value is LiveWorkspace {
  return (
    isRecord(value) &&
    typeof value.workspace === "string" &&
    isNullableString(value.task) &&
    typeof value.worker === "string" &&
    typeof value.base === "string" &&
    Array.isArray(value.files) &&
    value.files.every(isChangedFile) &&
    typeof value.summary === "string" &&
    typeof value.last_activity_at === "number" &&
    Array.isArray(value.checkpoints) &&
    value.checkpoints.every(isCheckpoint) &&
    isNullableString(value.proposal) &&
    typeof value.stalled === "boolean" &&
    isNullableBoolean(value.edits_after_version) &&
    (value.check_run === null || isCheckRun(value.check_run)) &&
    // A recorded frame or an older daemon sends no `activity`, which reads as nothing to say.
    (value.activity === undefined ||
      value.activity === null ||
      isActivity(value.activity))
  );
}

const ACTIVITY_STATES = everyValue<components["schemas"]["ActivityState"]>({
  working: true,
  silent: true,
  long_call: true,
  stopped: true,
  failed: true,
  waits_for_slot: true,
  waits_for_network: true,
  waits_for_usage_limit: true,
  parked: true,
});

const CALLS = everyValue<components["schemas"]["LiveCall"]>({
  command: true,
  fold: true,
  subagent: true,
  question: true,
  tool: true,
});

const SLOTS = everyValue<components["schemas"]["LiveSlot"]>({
  task_worker: true,
  reviewer: true,
});

function isActivity(value: unknown): boolean {
  return (
    isRecord(value) &&
    ACTIVITY_STATES.includes(value.state) &&
    (value.call === null || CALLS.includes(value.call)) &&
    (value.slot === null || SLOTS.includes(value.slot)) &&
    typeof value.since === "number" &&
    typeof value.supervised === "boolean" &&
    Array.isArray(value.told) &&
    value.told.every((name) => typeof name === "string") &&
    // A recorded frame or an older daemon sends no park.
    (value.parked_by === undefined || isNullableString(value.parked_by)) &&
    (value.wait_for === undefined || isNullableString(value.wait_for))
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
    case "worker_output":
      return (
        typeof value.agent === "string" &&
        typeof value.step === "string" &&
        isNullableString(value.tool) &&
        (value.text_length === null || isCount(value.text_length))
      );
    case "message_pushed":
      return (
        typeof value.message === "string" &&
        typeof value.thread === "string" &&
        typeof value.agent === "string"
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
    edits_after_version: frame.edits_after_version,
    check_run: frame.check_run,
    activity: frame.activity,
  };
}
