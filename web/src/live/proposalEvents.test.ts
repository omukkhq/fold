import { QueryClient } from "@tanstack/react-query";
import { beforeEach, describe, expect, it } from "vitest";
import { keys } from "../api/keys";
import { applyEvent } from "./apply";
import { isEventEnvelope, type EventEnvelope } from "./envelope";

const slug = "demo";
const revisions = [...keys.repository(slug), "revisions"];
const watched = [
  keys.proposals(slug),
  keys.proposal(slug, "P-4"),
  keys.proposalComments(slug, "P-4"),
  keys.proposal(slug, "P-5"),
  keys.proposalComments(slug, "P-5"),
  revisions,
  keys.board(slug),
];

let client: QueryClient;

beforeEach(() => {
  client = new QueryClient();
  for (const key of watched) client.setQueryData(key, { cached: true });
});

function stale(): string[] {
  return watched
    .filter((key) => client.getQueryState(key)?.isInvalidated === true)
    .map((key) => key.join("/"));
}

function event(type: string, fields: Record<string, unknown>): EventEnvelope {
  const frame: unknown = {
    kind: "event",
    seq: 7,
    type,
    actor: "review/quality",
    task: "T-3",
    proposal: null,
    workspace: null,
    revision: null,
    data: {},
    created_at: 1_760_000_000_000,
    ...fields,
  };
  if (!isEventEnvelope(frame)) throw new Error("not a frame");
  return frame;
}

describe("proposal keys and the event stream", () => {
  it.each([
    "proposal.opened",
    "proposal.version_added",
    "proposal.human_edited",
    "proposal.reviewer_fixed",
    "proposal.review_again_requested",
    "proposal.author_changed",
    "proposal.closed",
    "review.submitted",
    "review.comment_added",
  ])(
    "%s refreshes the list and that proposal with its comments, and no other proposal",
    (type) => {
      applyEvent(client, slug, event(type, { proposal: "P-4" }));
      expect(stale()).toEqual(
        expect.arrayContaining([
          `${slug}/proposals`,
          `${slug}/proposal/P-4`,
          `${slug}/proposal/P-4/comments`,
          `${slug}/board`,
        ]),
      );
      expect(stale()).not.toContain(`${slug}/proposal/P-5`);
    },
  );

  it("a conflict refreshes the proposal, so the detail screen switches to its conflict report", () => {
    applyEvent(
      client,
      slug,
      event("integration.conflicted", { proposal: "P-4", revision: "r8" }),
    );
    expect(stale()).toEqual(
      expect.arrayContaining([
        `${slug}/proposals`,
        `${slug}/proposal/P-4`,
        `${slug}/board`,
      ]),
    );
    expect(stale()).not.toContain(`${slug}/proposal/P-5`);
  });

  it("an integration refreshes the proposal and the revisions that name its new revision", () => {
    applyEvent(
      client,
      slug,
      event("proposal.integrated", { proposal: "P-4", revision: "r9" }),
    );
    expect(stale()).toEqual(
      expect.arrayContaining([
        `${slug}/proposals`,
        `${slug}/proposal/P-4`,
        revisions.join("/"),
      ]),
    );
    client = new QueryClient();
    for (const key of watched) client.setQueryData(key, { cached: true });
    applyEvent(
      client,
      slug,
      event("integration.succeeded", { proposal: "P-4", revision: "r9" }),
    );
    expect(stale()).toContain(revisions.join("/"));
  });

  it("main.advanced refreshes the repository keys and no proposal", () => {
    applyEvent(
      client,
      slug,
      event("main.advanced", { task: null, revision: "r9" }),
    );
    expect(stale()).toContain(revisions.join("/"));
    expect(stale().filter((k) => k.includes("proposal"))).toEqual([]);
  });

  it("the end of a check run refreshes its proposal and the run's own logs", () => {
    const run = keys.checkRun(slug, "cr-9");
    client.setQueryData(run, { cached: true });
    applyEvent(
      client,
      slug,
      event("checks.finished", {
        proposal: "P-4",
        data: { run: "cr-9", state: "failed" },
      }),
    );
    expect(stale()).toEqual(
      expect.arrayContaining([
        `${slug}/proposals`,
        `${slug}/proposal/P-4`,
        `${slug}/board`,
      ]),
    );
    expect(stale()).not.toContain(`${slug}/proposal/P-5`);
    expect(client.getQueryState(run)?.isInvalidated).toBe(true);
    client = new QueryClient();
    for (const key of watched) client.setQueryData(key, { cached: true });
    applyEvent(client, slug, event("checks.started", { data: {} }));
    expect(stale()).toEqual([`${slug}/proposals`, `${slug}/board`]);
  });

  it("an integration run refreshes every proposal page, since a batch's run shows on each member", () => {
    for (const type of ["checks.started", "checks.finished"]) {
      client = new QueryClient();
      for (const key of watched) client.setQueryData(key, { cached: true });
      applyEvent(
        client,
        slug,
        event(type, {
          proposal: "P-5",
          data: { run: "cr-9", stage: "integration", batch: "rb-2" },
        }),
      );
      expect(stale(), type).toEqual(
        expect.arrayContaining([
          `${slug}/proposals`,
          `${slug}/proposal/P-4`,
          `${slug}/proposal/P-5`,
        ]),
      );
    }
  });
});
