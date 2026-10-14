import type {
  EventEnvelope,
  LiveWorkspace,
  StoredEvent,
} from "../src/live/envelope";

const KEEP_ALIVE_MS = 15_000;
const STEP_MS = 1_500;
const T0 = Date.UTC(2026, 9, 1, 9, 0, 0);

function workspace(
  id: string,
  task: string,
  changed: number,
  at: number,
): LiveWorkspace {
  return {
    workspace: id,
    task,
    worker: "frontend/ui",
    base: "r1",
    files: Array.from({ length: changed }, (_, i) => ({
      path: `src/board/part${String(i + 1)}.ts`,
      previous_path: null,
      change: "modified" as const,
      added_lines: 3,
      removed_lines: 1,
      binary: false,
    })),
    summary: `${String(changed)} files, +${String(changed * 3)} -${String(changed)}`,
    last_activity_at: at,
    checkpoints: [],
    proposal: null,
  };
}

function stored(
  seq: number,
  type: StoredEvent["type"],
  task: string,
  data: StoredEvent["data"],
): StoredEvent {
  return {
    kind: "event",
    seq,
    type,
    actor: "frontend/ui",
    task,
    proposal: null,
    workspace: null,
    revision: null,
    data,
    created_at: T0 + seq * STEP_MS,
  };
}

/** Each frame follows the stored event numbered `after`; stored events are numbered without holes. */
const script: { after: number; frame: EventEnvelope }[] = [
  {
    after: 0,
    frame: {
      kind: "workspace_changed",
      ...workspace("ws-1", "T-1", 2, T0),
    },
  },
  {
    after: 1,
    frame: stored(1, "task.status_changed", "T-1", { status: "in_progress" }),
  },
  {
    after: 1,
    frame: {
      kind: "workspace_changed",
      ...workspace("ws-1", "T-1", 5, T0 + STEP_MS),
    },
  },
  {
    after: 2,
    frame: stored(2, "task.status_changed", "T-1", { status: "in_review" }),
  },
];

/** A live frame has no `id` line, because the daemon never stores it. */
function frame(event: object, id: number | null): string {
  const idLine = id === null ? "" : `id: ${String(id)}\n`;
  return `${idLine}event: message\ndata: ${JSON.stringify(event)}\n\n`;
}

function pause(ms: number, signal: AbortSignal): Promise<boolean> {
  return new Promise((resolve) => {
    if (signal.aborted) {
      resolve(false);
      return;
    }
    const timer = setTimeout(() => {
      resolve(true);
    }, ms);
    signal.addEventListener("abort", () => {
      clearTimeout(timer);
      resolve(false);
    });
  });
}

/** A scripted stream: a snapshot, a few events after the resume cursor, then keep-alive comments. */
export function fixtureEventTransport(
  slug: string,
  stepMs: number = STEP_MS,
  keepAliveMs: number = KEEP_ALIVE_MS,
) {
  return (lastSeq: number | null, signal: AbortSignal) => {
    async function* chunks(): AsyncGenerator<string> {
      const cursor = lastSeq ?? 0;
      const snapshot: EventEnvelope = {
        kind: "snapshot",
        seq: cursor,
        workspaces: [
          workspace("ws-1", "T-1", 1, T0),
          workspace("ws-2", "T-2", 0, T0),
        ],
      };
      yield frame(snapshot, cursor);
      const after = script.filter(({ after, frame }) =>
        frame.kind === "event" ? frame.seq > cursor : after >= cursor,
      );
      for (const { frame: event } of after) {
        if (!(await pause(stepMs, signal))) {
          return;
        }
        yield frame(event, event.kind === "event" ? event.seq : null);
      }
      while (await pause(keepAliveMs, signal)) {
        yield ": keep-alive\n\n";
      }
    }
    return Promise.resolve(chunks());
  };
}
