import { showsApproveLine, type HumanAction } from "./actions";

/** `You review v2 as tanim`: the version the buttons act on and the name the verdict goes out under. */
export function reviewerLine(version: number, human: string): string {
  return `You review v${String(version)} as ${human}`;
}

/** One line under the buttons that says why a button cannot work, or what a human edit keeps. */
export interface BarNote {
  kind: "approve" | "integrate" | "kept";
  text: string;
}

/**
 * The lines under the decision buttons, in the order the buttons stand. A reason both Approve and Integrate share shows once, and Approve's button then points to Integrate's line.
 */
export function barNotes(
  visible: readonly HumanAction[],
  approveBlock: string | null,
  integrateBlock: string | null,
  kept: string | null,
): BarNote[] {
  const notes: BarNote[] = [];
  const integrate = visible.includes("integrate");
  if (
    visible.includes("approve") &&
    approveBlock !== null &&
    showsApproveLine(approveBlock, integrateBlock, integrate)
  ) {
    notes.push({ kind: "approve", text: approveBlock });
  }
  if (integrate && integrateBlock !== null) {
    notes.push({ kind: "integrate", text: integrateBlock });
  }
  if (kept !== null) notes.push({ kind: "kept", text: kept });
  return notes;
}

/** Which line a disabled Approve names as its reason: its own line, or Integrate's when they share one; none while Approve can work. */
export function approveReason(
  notes: readonly BarNote[],
  approveBlock: string | null,
): BarNote["kind"] | null {
  if (approveBlock === null) return null;
  return notes.some((note) => note.kind === "approve")
    ? "approve"
    : "integrate";
}

/** A phone keeps Approve and Request changes in its bottom bar; the other decisions and Ask the author wait under More. */
export function phoneMore(visible: readonly HumanAction[]): HumanAction[] {
  return visible.filter(
    (action) => action === "integrate" || action === "review_again",
  );
}
