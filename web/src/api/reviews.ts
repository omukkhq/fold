import { fromFixture } from "./fixture";

/** No review-checkout route exists yet, so the schema has no type for it; this is the one local declaration of the row's shape. */
export interface ReviewCheckout {
  /** @example ws-12 */
  workspace: string;
  /** The checkout's directory in the project, `ws/review-p-<n>`. */
  path: string;
  /** @example P-4 */
  proposal: string;
  /** The proposal version the checkout holds. */
  version: number;
  /** The review worker label, `agent` or `agent/worker`. */
  worker: string;
  /** The last command the worker ran in the checkout; null before the first. */
  last_command: ReviewCommand | null;
}

export interface ReviewCommand {
  /** The result as the worker reported it, ids and counts only. */
  result: string;
  /** UTC milliseconds. */
  at: number;
}

const fixture = import.meta.env.DEV
  ? () => import("../../fixtures/reviews")
  : undefined;

/** The live review checkouts of the project, oldest first. */
export async function listReviewCheckouts(
  slug: string,
): Promise<ReviewCheckout[]> {
  return fromFixture(fixture, (m) => m.checkouts(slug), []);
}
