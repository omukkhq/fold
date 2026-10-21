import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import {
  proposalDetail,
  proposalSummary,
  proposalVersion,
  reviewComment,
} from "../test/proposals";
import { isApiError } from "./errors";
import { keys } from "./keys";
import {
  addComment,
  editProposalFile,
  getProposal,
  getProposalChanges,
  getProposalFile,
  getProposalFileDiff,
  integrateProposal,
  listComments,
  listProposals,
  sendForReviewAgain,
  submitReview,
} from "./proposals";
import { comparedFile, comparison, fileDiff } from "../test/diff";
import {
  proposalChangesQuery,
  proposalCommentsQuery,
  proposalFileDiffQuery,
  proposalFileQuery,
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
    expect(list.proposals.map((p) => p.id)).toEqual(["P-1", "P-2"]);
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

describe("proposal diff reads", () => {
  const FILE = comparedFile("src/lib.rs", "modified");

  it("reads a version's full change and the change since an earlier version", async () => {
    const calls = stubFetch({
      [`GET ${P}/P-4/diff`]: { body: { view: "files", ...comparison([FILE]) } },
    });
    const full = await getProposalChanges("demo", "P-4", {
      version: 2,
      since: null,
    });
    expect(full.files).toEqual([FILE]);
    await getProposalChanges("demo", "P-4", { version: 2, since: 1 });
    expect(calls.map((c) => Object.fromEntries(c.search))).toEqual([
      { version: "2" },
      { version: "2", since: "1" },
    ]);
  });

  it("reads one file's hunks of the range", async () => {
    const calls = stubFetch({
      [`GET ${P}/P-4/diff`]: { body: { view: "file", ...fileDiff(FILE, []) } },
    });
    const diff = await getProposalFileDiff(
      "demo",
      "P-4",
      { version: 2, since: 1 },
      "src/lib.rs",
    );
    expect(diff.path).toBe("src/lib.rs");
    expect(Object.fromEntries(calls[0]?.search ?? [])).toEqual({
      version: "2",
      since: "1",
      path: "src/lib.rs",
    });
  });

  it("refuses an answer of the other shape and rejects with the daemon's code", async () => {
    stubFetch({
      [`GET ${P}/P-4/diff`]: { body: { view: "file", ...fileDiff(FILE, []) } },
      [`GET ${P}/P-5/diff`]: { body: { view: "files", ...comparison([FILE]) } },
      [`GET ${P}/P-6/diff`]: errorReply(400, "invalid_request"),
    });
    const range = { version: 2, since: null };
    await expect(getProposalChanges("demo", "P-4", range)).rejects.toSatisfy(
      (e: unknown) => isApiError(e, "internal_error"),
    );
    await expect(
      getProposalFileDiff("demo", "P-5", range, "src/lib.rs"),
    ).rejects.toSatisfy((e: unknown) => isApiError(e, "internal_error"));
    await expect(getProposalChanges("demo", "P-6", range)).rejects.toSatisfy(
      (e: unknown) => isApiError(e, "invalid_request"),
    );
  });

  it("keys a range by its version numbers, outside the proposal", () => {
    const range = { version: 2, since: 1 };
    expect(proposalChangesQuery("demo", "P-4", range).queryKey).toEqual(
      keys.proposalChanges("demo", "P-4", 2, 1),
    );
    expect(
      proposalFileDiffQuery("demo", "P-4", range, "a.rs").queryKey,
    ).toEqual(keys.proposalFileDiff("demo", "P-4", 2, 1, "a.rs"));
    expect(keys.proposalChanges("demo", "P-4", 2, 1)[1]).toBe("proposal-diff");
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
      version: 2,
      path: "migrations/0004_reports.sql",
      side: "new",
      start_line: 2,
      end_line: 4,
      body: "Defaults please.",
      reply_to: null,
    });
    expect(stored.id).toBe("rc-9");
    expect(calls[0]?.body).toEqual({
      version: 2,
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
      version: 2,
      verdict: "approve",
      body: "ok",
      comments: [],
    });
    expect(answer.version).toBe(2);
    expect(calls[0]?.body).toEqual({
      version: 2,
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

describe("human edits", () => {
  it("reads a file of a version, keyed outside the proposal", async () => {
    const file = {
      path: "src/lib.rs",
      kind: "file" as const,
      hash: "a".repeat(64),
      size: 3,
      binary: false,
      too_large: false,
      text: "x\n",
    };
    const calls = stubFetch({ [`GET ${P}/P-4/file`]: { body: file } });
    await expect(
      getProposalFile("demo", "P-4", 2, "src/lib.rs"),
    ).resolves.toEqual(file);
    expect(calls[0]?.search.get("version")).toBe("2");
    expect(calls[0]?.search.get("path")).toBe("src/lib.rs");
    expect(proposalFileQuery("demo", "P-4", 2, "src/lib.rs").queryKey).toEqual([
      "demo",
      "proposal-file",
      "P-4",
      2,
      "src/lib.rs",
    ]);
  });

  it("posts an edit of the version read and answers with the new version", async () => {
    const calls = stubFetch({
      [`POST ${P}/P-4/edits`]: (request) =>
        (request.body as { version: number }).version === 2
          ? {
              status: 201,
              body: proposalVersion(3, {
                created_by: "human:tanim",
                human_edit: true,
              }),
            }
          : errorReply(409, "stale_version"),
    });
    const made = await editProposalFile("demo", "P-4", {
      version: 2,
      path: "src/lib.rs",
      content: "y\n",
    });
    expect([made.version, made.human_edit]).toEqual([3, true]);
    expect(calls[0]?.body).toEqual({
      version: 2,
      path: "src/lib.rs",
      content: "y\n",
    });
    await expect(
      editProposalFile("demo", "P-4", {
        version: 1,
        path: "src/lib.rs",
        content: "y\n",
      }),
    ).rejects.toSatisfy((e: unknown) => isApiError(e, "stale_version"));
  });

  it("sends a version for review again", async () => {
    const calls = stubFetch({
      [`POST ${P}/P-4/review-again`]: {
        body: proposalSummary("P-4", { current_version: 3 }),
      },
    });
    const summary = await sendForReviewAgain("demo", "P-4", 3);
    expect(summary.current_version).toBe(3);
    expect(calls[0]?.body).toEqual({ version: 3 });
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
