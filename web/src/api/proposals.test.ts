import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import {
  proposalDetail,
  proposalSummary,
  reviewComment,
} from "../test/proposals";
import { isApiError } from "./errors";
import { keys } from "./keys";
import {
  addComment,
  getProposal,
  integrateProposal,
  listComments,
  listProposals,
  submitReview,
} from "./proposals";
import {
  proposalCommentsQuery,
  proposalQuery,
  proposalsQuery,
} from "./queries";

afterEach(() => {
  vi.unstubAllGlobals();
});

const P = "/api/v1/projects/demo/proposals";

describe("proposal reads", () => {
  it("lists every proposal of the project", async () => {
    const calls = stubFetch({
      [`GET ${P}`]: {
        body: { proposals: [proposalSummary("P-1"), proposalSummary("P-2")] },
      },
    });
    const list = await listProposals("demo");
    expect(list.map((p) => p.id)).toEqual(["P-1", "P-2"]);
    expect(calls[0]?.search.toString()).toBe("");
  });

  it("reads a proposal, and answers null for one the project does not have", async () => {
    const detail = proposalDetail(proposalSummary("P-4"));
    stubFetch({
      [`GET ${P}/P-4`]: { body: detail },
      [`GET ${P}/P-99`]: errorReply(404, "proposal_not_found"),
      [`GET ${P}/P-5`]: errorReply(401, "human_name_required"),
    });
    await expect(getProposal("demo", "P-4")).resolves.toEqual(detail);
    await expect(getProposal("demo", "P-99")).resolves.toBeNull();
    await expect(getProposal("demo", "P-5")).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "human_name_required"),
    );
  });

  it("reads every comment of every version", async () => {
    const calls = stubFetch({
      [`GET ${P}/P-4/comments`]: {
        body: { comments: [reviewComment(1, 1, { outdated: true })] },
      },
    });
    const comments = await listComments("demo", "P-4");
    expect(comments.map((c) => [c.id, c.outdated])).toEqual([["rc-1", true]]);
    expect(calls[0]?.search.has("version")).toBe(false);
  });
});

describe("human writes", () => {
  it("posts a comment with its range exactly as the selection gives it, and its reply", async () => {
    const calls = stubFetch({
      [`POST ${P}/P-4/comments`]: {
        status: 201,
        body: reviewComment(9, 2, { author: "human:tanim" }),
      },
    });
    const stored = await addComment("demo", "P-4", {
      path: "migrations/0004_reports.sql",
      side: "new",
      start_line: 2,
      end_line: 4,
      body: "Defaults please.",
      reply_to: null,
    });
    expect(stored.id).toBe("rc-9");
    expect(calls[0]?.body).toEqual({
      path: "migrations/0004_reports.sql",
      side: "new",
      start_line: 2,
      end_line: 4,
      body: "Defaults please.",
      reply_to: null,
    });
  });

  it("posts a verdict with its body and comments", async () => {
    const calls = stubFetch({
      [`POST ${P}/P-4/reviews`]: {
        status: 201,
        body: {
          proposal: "P-4",
          version: 2,
          verdict: "approve",
          comments: [],
          summary: proposalSummary("P-4", { current_version: 2 }),
        },
      },
    });
    const answer = await submitReview("demo", "P-4", {
      verdict: "approve",
      body: "ok",
      comments: [],
    });
    expect(answer.version).toBe(2);
    expect(calls[0]?.body).toEqual({
      verdict: "approve",
      body: "ok",
      comments: [],
    });
  });

  it("integrates with the version on screen as expected_version, and rejects with the daemon's code", async () => {
    const calls = stubFetch({
      [`POST ${P}/P-3/integrate`]: (request) =>
        (request.body as { expected_version: number }).expected_version === 1
          ? {
              body: { proposal: "P-3", result: "succeeded", revision: "r9" },
            }
          : errorReply(409, "stale_version"),
    });
    await expect(integrateProposal("demo", "P-3", 1)).resolves.toEqual({
      proposal: "P-3",
      result: "succeeded",
      revision: "r9",
    });
    expect(calls[0]?.body).toEqual({ expected_version: 1 });
    await expect(integrateProposal("demo", "P-3", 2)).rejects.toSatisfy(
      (e: unknown) => isApiError(e, "stale_version"),
    );
  });
});

describe("proposal queries", () => {
  it("key the list, the detail and its comments under the project, the comments under the detail", () => {
    expect(proposalsQuery("demo").queryKey).toEqual(keys.proposals("demo"));
    expect(proposalQuery("demo", "P-4").queryKey).toEqual(
      keys.proposal("demo", "P-4"),
    );
    expect(proposalCommentsQuery("demo", "P-4").queryKey).toEqual([
      ...keys.proposal("demo", "P-4"),
      "comments",
    ]);
  });
});
