import { api } from "./client";
import { unwrap } from "./result";
import type { components } from "./schema";

export type ReviewCheckout = components["schemas"]["ReviewCheckoutView"];

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
