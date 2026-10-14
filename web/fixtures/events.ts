import { isLive, type EventEnvelope } from "../src/live/envelope";

const KEEP_ALIVE_MS = 15_000;
const STEP_MS = 1_500;

/** Stored events are numbered without holes; a live `workspace.changed` repeats the number of the newest stored event before it. */
const script: EventEnvelope[] = [
  {
    seq: 0,
    type: "workspace.changed",
    workspace: 1,
    data: { state: { changed: 2, conflicts: 0 } },
  },
  {
    seq: 1,
    type: "task.status_changed",
    task: 1,
    data: { status: "in_progress" },
  },
  {
    seq: 1,
    type: "workspace.changed",
    workspace: 1,
    data: { state: { changed: 5, conflicts: 0 } },
  },
  {
    seq: 2,
    type: "task.status_changed",
    task: 1,
    data: { status: "in_review" },
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
      yield frame(
        {
          seq: cursor,
          type: "snapshot",
          workspaces: [
            { workspace: 1, state: { changed: 1, conflicts: 0 } },
            { workspace: 2, state: { changed: 0, conflicts: 0 } },
          ],
        },
        cursor,
      );
      const after = script.filter((e) =>
        isLive(e) ? e.seq >= cursor : e.seq > cursor,
      );
      for (const event of after) {
        if (!(await pause(stepMs, signal))) {
          return;
        }
        yield frame(event, isLive(event) ? null : event.seq);
      }
      while (await pause(keepAliveMs, signal)) {
        yield ": keep-alive\n\n";
      }
    }
    return Promise.resolve(chunks());
  };
}
