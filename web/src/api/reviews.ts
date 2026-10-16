import { api } from "./client";
import { unwrap } from "./result";
import type { components } from "./schema";

/** The daemon records no command run in a review checkout, so no route sends one; the row draws nothing for an absent one. */
export interface ReviewCommand {
  /** The result as the worker reported it, ids and counts only. */
  result: string;
  /** UTC milliseconds. */
  at: number;
}

export type ReviewCheckout = components["schemas"]["ReviewCheckoutView"] & {
  last_command?: ReviewCommand | null;
};

/** The active review checkouts of the project, oldest first. */
export async function listReviewCheckouts(
  slug: string,
): Promise<ReviewCheckout[]> {
  const { checkouts } = await unwrap(
    api.GET("/api/v1/projects/{slug}/reviews/checkouts", {
      params: { path: { slug } },
    }),
  );
  return checkouts;
}

export type ReviewBatch = components["schemas"]["ReviewBatch"];
export type ReviewBatchMember = components["schemas"]["ReviewBatchMember"];

/** The project's review batches in number order. */
export async function listReviewBatches(slug: string): Promise<ReviewBatch[]> {
  const { batches } = await unwrap(
    api.GET("/api/v1/projects/{slug}/reviews/batches", {
      params: { path: { slug } },
    }),
  );
  return batches;
}

/** One review batch. Rejects with `review_batch_not_found` for an unknown id. */
export function getReviewBatch(slug: string, id: string): Promise<ReviewBatch> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/reviews/batches/{id}", {
      params: { path: { slug, id } },
    }),
  );
}
