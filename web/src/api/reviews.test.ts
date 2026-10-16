import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "@/test/stubFetch";
import { isApiError } from "./errors";
import { batch } from "@/test/workers";
import {
  getReviewBatch,
  listReviewBatches,
  listReviewCheckouts,
} from "./reviews";

afterEach(() => {
  vi.unstubAllGlobals();
});

const CHECKOUT = {
  workspace: "ws-12",
  path: "ws/review-p-4",
  proposal: "P-4",
  version: 2,
  worker: "review/quality",
  base: "r7",
  created_at: 1,
};

describe("listReviewCheckouts", () => {
  it("lists the active checkouts the daemon answers", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/reviews/checkouts": {
        body: { checkouts: [CHECKOUT] },
      },
    });
    const rows = await listReviewCheckouts("demo");
    expect(rows).toEqual([CHECKOUT]);
  });

  it("rejects with the daemon's code", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/reviews/checkouts": errorReply(
        404,
        "project_not_found",
      ),
    });
    await expect(listReviewCheckouts("demo")).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "project_not_found"),
    );
  });
});

describe("review batches", () => {
  it("lists the batches and reads one", async () => {
    const one = batch();
    stubFetch({
      "GET /api/v1/projects/demo/reviews/batches": { body: { batches: [one] } },
      "GET /api/v1/projects/demo/reviews/batches/rb-3": { body: one },
    });
    expect(await listReviewBatches("demo")).toEqual([one]);
    expect(await getReviewBatch("demo", "rb-3")).toEqual(one);
  });

  it("rejects an unknown batch with its code", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/reviews/batches/rb-9": errorReply(
        404,
        "review_batch_not_found",
      ),
    });
    await expect(getReviewBatch("demo", "rb-9")).rejects.toSatisfy(
      (e: unknown) => isApiError(e, "review_batch_not_found"),
    );
  });
});
