import { parseId } from "../../format/ids";

export interface ComparePair {
  /** The older revision, for example `r3`. */
  from: string;
  /** The newer revision, for example `r8`. */
  to: string;
}

export type CompareResult =
  { ok: true; pair: ComparePair } | { ok: false; reason: "same" | "invalid" };

function revisionNumber(id: string): number | null {
  const found = parseId(id);
  return found?.kind === "revision" ? found.number : null;
}

/** Puts the older revision first and rejects the same revision twice or a value that is not a revision id. */
export function normalizeCompare(a: string, b: string): CompareResult {
  const first = revisionNumber(a);
  const second = revisionNumber(b);
  if (first === null || second === null) {
    return { ok: false, reason: "invalid" };
  }
  if (first === second) {
    return { ok: false, reason: "same" };
  }
  return first < second
    ? {
        ok: true,
        pair: { from: `r${String(first)}`, to: `r${String(second)}` },
      }
    : {
        ok: true,
        pair: { from: `r${String(second)}`, to: `r${String(first)}` },
      };
}

/** The diff route for a pair. */
export function comparePath(slug: string, pair: ComparePair): string {
  return `/p/${slug}/diff?from=${pair.from}&to=${pair.to}`;
}

/** The other end of the comparison a revision opens with: the revision just before it, or just after it when it is the oldest. `revisions` is newest first; null when there is nothing to compare. */
export function defaultCompareFrom(
  revisions: readonly { id: string }[],
  selected: string,
): string | null {
  const at = revisions.findIndex((r) => r.id === selected);
  if (at === -1) {
    return null;
  }
  return revisions[at + 1]?.id ?? revisions[at - 1]?.id ?? null;
}

/** `Compare r7 → r8` for a valid pair, older first; `Compare` otherwise. */
export function compareLabel(a: string, b: string): string {
  const result = normalizeCompare(a, b);
  return result.ok
    ? `Compare ${result.pair.from} → ${result.pair.to}`
    : "Compare";
}
