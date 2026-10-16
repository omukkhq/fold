import { isApiError } from "@/api/errors";
import type { ThreadView } from "@/api/messages";
import { findThread } from "./filters";

/** The thread pane's content: a thread of the listing, one read on its own because the listing did not reach it, or the newest visible thread when the address names none. */
export type OpenThread =
  | { kind: "listed"; thread: ThreadView | null }
  | { kind: "unlisted"; id: string };

/**
 * A requested thread that no listed thread holds is read directly: opening the first thread instead would show another conversation under the requested address.
 * A requested thread that the filters hide falls back to the first visible thread, as the filters ask.
 */
export function openThread(
  all: ThreadView[],
  visible: ThreadView[],
  requested: string | undefined,
): OpenThread {
  if (requested !== undefined && findThread(all, requested) === null) {
    return { kind: "unlisted", id: requested };
  }
  return {
    kind: "listed",
    thread: findThread(visible, requested) ?? visible[0] ?? null,
  };
}

/** A malformed id names no message either, so it reads as missing rather than as a daemon failure. */
export function isMissingThread(error: unknown): boolean {
  return (
    isApiError(error, "message_not_found") || isApiError(error, "invalid_id")
  );
}
