export type LiveState = Record<string, unknown>;

export interface WorkspaceLive {
  workspace: number;
  state: LiveState;
}

/** The one declaration of the stream's frame shape in web/src, holding only the fields the stream and the cache rules read. */
export interface EventEnvelope {
  seq: number;
  type: string;
  task?: number;
  proposal?: number;
  workspace?: number;
  data?: Record<string, unknown>;
  workspaces?: WorkspaceLive[];
}

export type SnapshotEnvelope = EventEnvelope & {
  type: "snapshot";
  workspaces: WorkspaceLive[];
};

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isWorkspaceLive(value: unknown): value is WorkspaceLive {
  return (
    isRecord(value) &&
    typeof value.workspace === "number" &&
    isRecord(value.state)
  );
}

function optionalNumber(value: unknown): boolean {
  return value === undefined || typeof value === "number";
}

/** An event of an unknown type passes; `apply.ts` ignores it while its `seq` still advances the cursor. */
export function isEventEnvelope(value: unknown): value is EventEnvelope {
  if (!isRecord(value)) {
    return false;
  }
  if (
    typeof value.seq !== "number" ||
    !Number.isInteger(value.seq) ||
    value.seq < 0 ||
    typeof value.type !== "string" ||
    value.type === ""
  ) {
    return false;
  }
  if (
    !optionalNumber(value.task) ||
    !optionalNumber(value.proposal) ||
    !optionalNumber(value.workspace)
  ) {
    return false;
  }
  if (value.data !== undefined && !isRecord(value.data)) {
    return false;
  }
  if (value.type === "snapshot") {
    return (
      Array.isArray(value.workspaces) && value.workspaces.every(isWorkspaceLive)
    );
  }
  return true;
}

/** `workspace.changed` and `snapshot` carry live workspace state, which the daemon never stores. */
export function isLive(event: EventEnvelope): boolean {
  return event.type === "workspace.changed" || event.type === "snapshot";
}

export function isSnapshot(event: EventEnvelope): event is SnapshotEnvelope {
  return event.type === "snapshot" && Array.isArray(event.workspaces);
}
