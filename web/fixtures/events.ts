import type {
  EventEnvelope,
  LiveWorkspace,
  StoredEvent,
} from "../src/live/envelope";
import { workspaces } from "./workspaces";

const KEEP_ALIVE_MS = 15_000;
const STEP_MS = 1_500;
const T0 = Date.UTC(2026, 9, 1, 9, 0, 0);

const active = workspaces.flatMap((entry) =>
  entry.live === null ? [] : [entry.live],
);

/** The listed live state of `id` with `extra` more changed files, as the watcher reports it a moment later. */
function changed(id: string, extra: number, after: number): LiveWorkspace {
  const base = active.find((live) => live.workspace === id);
  if (base === undefined) {
    throw new Error(`no active fixture workspace ${id}`);
  }
  const files = [
    ...base.files,
    ...Array.from({ length: extra }, (_, i) => ({
      path: `src/forms/step${String(i + 1)}.ts`,
      previous_path: null,
      change: "added" as const,
      added_lines: 12,
      removed_lines: 0,
      binary: false,
    })),
  ];
  return {
    ...base,
    files,
    summary: `${String(files.length)} files`,
    last_activity_at: base.last_activity_at + after,
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
      ...changed("ws-1", 1, STEP_MS),
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
      ...changed("ws-1", 2, 2 * STEP_MS),
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
        workspaces: slug === "demo" ? active : [],
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
