import { isApiError } from "../api/errors";
import { nextDelay } from "./backoff";
import { isEventEnvelope, type EventEnvelope } from "./envelope";
import { createSseParser } from "./sse-parse";

/** Three keep-alive periods of 15 seconds. */
export const DEAD_AFTER_MS = 45_000;

/** Opens one connection that resumes after `lastSeq`; the returned chunks end or throw when the connection ends. */
export type Transport = (
  lastSeq: number | null,
  signal: AbortSignal,
) => Promise<AsyncIterable<string>>;

export interface StreamHandlers {
  /** The first frame of a connection arrived. */
  onOpen(): void;
  onEvent(event: EventEnvelope): void;
  /** The connection ended or went silent; a new attempt follows after `delayMs` unless `retry` comes first. */
  onDrop(delayMs: number): void;
  /** Stored events may be missing from what was applied: a sequence number was skipped, or the daemon refused the cursor. */
  onResync(): void;
  /** The daemon answered `auth_required`: the session ended off loopback. The stream stops and makes no further attempt. */
  onAuthRequired?(): void;
}

export interface StreamOptions {
  transport: Transport;
  handlers: StreamHandlers;
  random?: () => number;
  /** The newest stored event the caller already applied, so the first connection resumes after it. */
  lastSeq?: number | null;
}

export interface StreamHandle {
  stop(): void;
  /** The newest stored event seen so far, or null before any. */
  seq(): number | null;
  /** Starts the next attempt now instead of at the end of the backoff delay; does nothing while a connection is open. */
  retry(): void;
}

/**
 * Stored events are numbered without holes, so a jump past `lastSeq + 1` means one was lost. Live frames are not stored:
 * a `workspace_changed`, `agent_presence_changed` or `worker_output` frame has no `seq` and never moves the cursor, and a snapshot moves it to the newest stored event it reports.
 */
export function startStream(options: StreamOptions): StreamHandle {
  const { transport, handlers } = options;
  const random = options.random ?? Math.random;
  let stopped = false;
  const isStopped = () => stopped;
  let lastSeq: number | null = options.lastSeq ?? null;
  let current: AbortController | null = null;
  let wake: (() => void) | null = null;

  function accept(data: string): void {
    let parsed: unknown;
    try {
      parsed = JSON.parse(data);
    } catch {
      return;
    }
    if (!isEventEnvelope(parsed)) {
      return;
    }
    if (parsed.kind === "snapshot") {
      lastSeq = Math.max(lastSeq ?? 0, parsed.seq);
      handlers.onEvent(parsed);
      return;
    }
    if (
      parsed.kind === "workspace_changed" ||
      parsed.kind === "agent_presence_changed" ||
      parsed.kind === "worker_output"
    ) {
      handlers.onEvent(parsed);
      return;
    }
    if (lastSeq !== null && parsed.seq <= lastSeq) {
      return;
    }
    if (lastSeq !== null && parsed.seq > lastSeq + 1) {
      handlers.onResync();
    }
    lastSeq = parsed.seq;
    handlers.onEvent(parsed);
  }

  async function connect(controller: AbortController): Promise<boolean> {
    let idle: ReturnType<typeof setTimeout> | undefined;
    const aborted = new Promise<"aborted">((resolve) => {
      controller.signal.addEventListener("abort", () => {
        resolve("aborted");
      });
    });
    const arm = () => {
      clearTimeout(idle);
      idle = setTimeout(() => {
        controller.abort();
      }, DEAD_AFTER_MS);
    };
    let healthy = false;
    try {
      arm();
      const opened = await Promise.race([
        transport(lastSeq, controller.signal),
        aborted,
      ]);
      if (opened === "aborted") {
        return healthy;
      }
      const iterator = opened[Symbol.asyncIterator]();
      const parser = createSseParser();
      try {
        for (;;) {
          const step = await Promise.race([iterator.next(), aborted]);
          // A chunk that settles in the same turn as the abort belongs to a closed connection, and maybe to a project already left.
          if (step === "aborted" || step.done || controller.signal.aborted) {
            break;
          }
          arm();
          for (const frame of parser.push(step.value)) {
            if (!healthy) {
              healthy = true;
              handlers.onOpen();
            }
            if (frame.kind === "message") {
              accept(frame.data);
            }
          }
        }
      } finally {
        // A generator suspended in `next()` queues `return()` behind it, so it is not awaited.
        void iterator.return?.().catch(() => undefined);
      }
    } catch (error) {
      if (isApiError(error, "auth_required") && !isStopped()) {
        stopped = true;
        handlers.onAuthRequired?.();
      } else if (isApiError(error, "invalid_last_event_id") && !isStopped()) {
        lastSeq = null;
        handlers.onResync();
      }
    } finally {
      clearTimeout(idle);
      controller.abort();
    }
    return healthy;
  }

  async function run(): Promise<void> {
    let attempt = 0;
    while (!isStopped()) {
      const controller = new AbortController();
      current = controller;
      const healthy = await connect(controller);
      if (isStopped()) {
        return;
      }
      if (healthy) {
        attempt = 0;
      }
      const delay = nextDelay(attempt, random);
      attempt += 1;
      handlers.onDrop(delay);
      await new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, delay);
        wake = () => {
          clearTimeout(timer);
          resolve();
        };
      });
      wake = null;
    }
  }

  void run();

  return {
    stop() {
      stopped = true;
      current?.abort();
      wake?.();
    },
    retry() {
      wake?.();
    },
    seq: () => lastSeq,
  };
}
