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

export type BatchIntegrationResult =
  components["schemas"]["BatchIntegrationResult"];

/** The signed-in human integrates the approved members of a submitted batch together. Rejects with `invalid_request` and `details.reason` `batch_waiting` while a member has no final state, `batch_integrated` once the batch's integration ended, `batch_not_submitted` or `integration_queued`. */
export function integrateReviewBatch(
  slug: string,
  id: string,
): Promise<BatchIntegrationResult> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/reviews/batches/{id}/integrate", {
      params: { path: { slug, id } },
    }),
  );
}
