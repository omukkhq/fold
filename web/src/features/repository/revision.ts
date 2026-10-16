import { isApiError } from "@/api/errors";
import type { Revision } from "@/api/repository";

/** Which revision the screen shows: one the listing holds, the newest when the address names none, or one the listing did not reach and that is read on its own. */
export type RevisionPick =
  | { kind: "listed"; id: string }
  | { kind: "unlisted"; id: string }
  | { kind: "newest" };

export function pickRevision(
  listed: readonly Revision[],
  requested: string | undefined,
): RevisionPick {
  if (requested === undefined) return { kind: "newest" };
  return listed.some((r) => r.id === requested)
    ? { kind: "listed", id: requested }
    : { kind: "unlisted", id: requested };
}

/** A malformed id names no revision either, so it reads as missing rather than as a daemon failure. */
export function isMissingRevision(error: unknown): boolean {
  return (
    isApiError(error, "revision_not_found") || isApiError(error, "invalid_id")
  );
}
