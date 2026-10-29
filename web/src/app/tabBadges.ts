import type { NavId } from "../routes/nav";

export interface TabBadge {
  count: number;
  /** What the count says to a screen reader. */
  label: string;
  /** What the count counts, for its tooltip. */
  hint: string;
}

/** A tab counts only what waits for the human, in gold: the proposals that wait for their review and the questions that wait for their answer, as the chat rows count them. */
export function tabBadge(
  id: NavId,
  reviews: number,
  questions: number,
): TabBadge | null {
  if (id === "proposals" && reviews > 0) {
    return {
      count: reviews,
      label: `${String(reviews)} ${reviews === 1 ? "proposal waits" : "proposals wait"} for your review`,
      hint: "Open proposals whose current version waits for your review",
    };
  }
  if (id === "messages" && questions > 0) {
    return {
      count: questions,
      label: `${String(questions)} ${questions === 1 ? "question waits" : "questions wait"} for your answer`,
      hint: "Questions that wait for your answer",
    };
  }
  return null;
}
