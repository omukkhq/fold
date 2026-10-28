import type { NavId } from "../routes/nav";

export interface TabBadge {
  count: number;
  /** What the count says to a screen reader. */
  label: string;
  /** What the count counts, for its tooltip. */
  hint: string;
}

/** A tab counts only what waits for the human, in gold: the proposals that wait for their review and the threads that wait for their answer. */
export function tabBadge(
  id: NavId,
  reviews: number,
  awaiting: number,
): TabBadge | null {
  if (id === "proposals" && reviews > 0) {
    return {
      count: reviews,
      label: `${String(reviews)} ${reviews === 1 ? "proposal waits" : "proposals wait"} for your review`,
      hint: "Open proposals whose current version waits for your review",
    };
  }
  if (id === "messages" && awaiting > 0) {
    return {
      count: awaiting,
      label: `${String(awaiting)} ${awaiting === 1 ? "thread waits" : "threads wait"} for your answer`,
      hint: "Threads with a question that waits for your answer",
    };
  }
  return null;
}
