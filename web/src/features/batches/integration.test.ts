import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import type { ReviewBatch } from "@/api/reviews";
import { batch } from "@/test/workers";
import { integrateFailure, integrationNote } from "./integration";

function at(
  integration: Partial<ReviewBatch["integration"]>,
  fields: Partial<ReviewBatch> = {},
): ReviewBatch {
  const base = batch(fields);
  return { ...base, integration: { ...base.integration, ...integration } };
}

describe("integrationNote", () => {
  it("says the members integrate together once the review is submitted, and not for a closed batch", () => {
    expect(integrationNote(batch())).toEqual({
      text: "When the review is submitted, fold integrates the approved members together, with one run of the integration checks.",
      canIntegrate: false,
    });
    expect(integrationNote(batch({ state: "closed" })).text).toBe(
      "The batch closed without a review, so each member integrates on its own.",
    );
  });

  it("names the members the batch waits for", () => {
    expect(
      integrationNote(at({ state: "waiting", pending: ["P-4", "P-6"] })),
    ).toEqual({
      text: "Waiting for P-4, P-6 to get a final verdict. Then fold integrates the approved members together.",
      canIntegrate: false,
    });
  });

  it("offers the integrate action only for a ready batch with members and no waiting request", () => {
    expect(
      integrationNote(at({ state: "ready", members: ["P-3", "P-5"] })),
    ).toEqual({
      text: "Ready: P-3, P-5 integrate together, with one run of the integration checks.",
      canIntegrate: true,
    });
    expect(
      integrationNote(
        at({ state: "ready", members: ["P-3", "P-5"], queued: true }),
      ),
    ).toEqual({
      text: "Queued: fold integrates P-3, P-5 together in its turn.",
      canIntegrate: false,
    });
    expect(integrationNote(at({ state: "ready", members: [] }))).toEqual({
      text: "No member can integrate with the batch. Each member integrates on its own.",
      canIntegrate: false,
    });
  });

  it("holds the integrate action while a member's next version runs its checks", () => {
    const updating = [
      {
        proposal: "P-3",
        run: { run: "cr-4", purpose: "update_proposal" as const },
      },
      { proposal: "P-5", run: { run: "cr-5", purpose: "review_fix" as const } },
    ];
    expect(
      integrationNote(
        at({ state: "ready", members: ["P-3", "P-5"], queued: true, updating }),
      ),
    ).toEqual({
      text: "Waiting for the checks of an update of P-3, a reviewer's fix of P-5 to end. Then fold integrates the approved members together.",
      canIntegrate: false,
    });
  });

  it("says how the integration ended", () => {
    expect(
      integrationNote(
        at({
          state: "ended",
          outcome: "together",
          revisions: [
            { proposal: "P-3", version: 2, revision: "r18" },
            { proposal: "P-5", version: 1, revision: "r19" },
          ],
        }),
      ).text,
    ).toBe("Integrated together: P-3 as r18, P-5 as r19.");
    expect(
      integrationNote(
        at({ state: "ended", outcome: "one_at_a_time", run: "cr-9" }),
      ).text,
    ).toBe(
      "The integration checks of the members together failed (cr-9), so fold integrated them one at a time.",
    );
    expect(
      integrationNote(at({ state: "ended", outcome: "one_at_a_time" })).text,
    ).toBe(
      "The integration checks of the members together failed, so fold integrated them one at a time.",
    );
    expect(integrationNote(at({ state: "ended", outcome: "none" })).text).toBe(
      "No member integrated with the batch. Each member integrates on its own.",
    );
  });
});

describe("integrateFailure", () => {
  it("names the refusal by its code and reason", () => {
    const refused = (reason: string) =>
      new ApiError("invalid_request", "refused", { reason });
    expect(integrateFailure(refused("batch_waiting"))).toBe(
      "A member has no final verdict yet.",
    );
    expect(integrateFailure(refused("batch_integrated"))).toBe(
      "The batch is integrated already.",
    );
    expect(integrateFailure(refused("batch_not_submitted"))).toBe(
      "The batch review is not submitted yet.",
    );
    expect(integrateFailure(refused("integration_queued"))).toBe(
      "The batch waits in the integration queue already.",
    );
    expect(integrateFailure(refused("something_else"))).toBe(
      "The daemon refused the request as written.",
    );
    expect(integrateFailure(new ApiError("head_moved", "moved"))).toBe(
      "main moved during each attempt. Try again.",
    );
    expect(integrateFailure(new ApiError("checks_file_invalid", "bad"))).toBe(
      "fold-checks.toml on main is not valid. Integrate again once it is fixed.",
    );
    const running = (details: Record<string, unknown>) =>
      new ApiError("update_checks_running", "running", details);
    expect(
      integrateFailure(running({ proposal: "P-5", purpose: "review_fix" })),
    ).toBe(
      "A reviewer's fix of P-5 is running its checks. Integrate when they end.",
    );
    expect(
      integrateFailure(
        running({ proposal: "P-3", purpose: "update_proposal" }),
      ),
    ).toBe("An update of P-3 is running its checks. Integrate when they end.");
    expect(integrateFailure(running({}))).toBe(
      "An update of a member is running its checks. Integrate when they end.",
    );
    expect(integrateFailure(new Error("offline"))).toBe(
      "The request did not reach the daemon. Try again.",
    );
  });
});
