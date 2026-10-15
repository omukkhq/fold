import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "@/test/stubFetch";
import { isApiError } from "./errors";
import { listReviewCheckouts } from "./reviews";

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
  it("lists the active checkouts the daemon answers, with no last command", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/reviews/checkouts": {
        body: { checkouts: [CHECKOUT] },
      },
    });
    const rows = await listReviewCheckouts("demo");
    expect(rows).toEqual([CHECKOUT]);
    expect(rows[0]?.last_command).toBeUndefined();
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
