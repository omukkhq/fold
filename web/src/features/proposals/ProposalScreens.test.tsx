// @vitest-environment jsdom
import { byContent } from "@/test/content";
import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { RouterProvider, createMemoryHistory } from "@tanstack/react-router";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Comparison, FileDiff } from "@/api/diff";
import type {
  ProposalDetail,
  ProposalSummary,
  ReviewComment,
} from "@/api/proposals";
import type { Revision } from "@/api/repository";
import { createQueryClient } from "@/app/queryClient";
import { createAppRouter } from "@/app/router";
import { applyEvent } from "@/live/apply";
import { isEventEnvelope } from "@/live/envelope";
import { daemon, ok, refuse, type Call } from "@/test/daemon";
import { expectMentionPopover } from "@/test/mentionBox";
import { mentionRoutes } from "@/test/mentionRoutes";
import {
  conflictReport,
  humanGates,
  proposalDetail,
  proposalSummary,
  proposalVersion,
  review,
  reviewComment,
  revision,
} from "@/test/proposals";
import { ThemeProvider } from "@/theme/ThemeProvider";

vi.mock("@/api/client", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/api/client")>();
  const { daemon: fake } = await import("@/test/daemon");
  return { ...real, api: fake.api };
});

const SCHEMA = "migrations/0004_reports.sql";

// The version diff is the diff seam's to read; the proposal screens only name the states it compares.
const diffReads = vi.hoisted(() => ({ refs: [] as string[] }));
vi.mock("@/api/diff", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/diff")>();
  const { add, comparedFile, comparison, fileDiff, hunk, state } = await import(
    "@/test/diff"
  );
  const lines = [
    "CREATE TABLE reports (",
    "  id INTEGER,",
    "  owner_id INTEGER,",
    "  report_date TEXT",
    ");",
  ];
  const file = comparedFile("migrations/0004_reports.sql", "added", {
    added_lines: lines.length,
    removed_lines: 0,
    new_hash: "f".repeat(64),
  });
  return {
    ...actual,
    getComparison: (
      _slug: string,
      from: string | undefined,
      to: string,
    ): Promise<Comparison> => {
      diffReads.refs.push(`${from ?? ""}..${to}`);
      return Promise.resolve(
        comparison([file], {
          source: "recorded",
          from: state("changeset", from ?? ""),
          to: state("changeset", to),
        }),
      );
    },
    getFileDiff: (
      _slug: string,
      _from: string | undefined,
      _to: string,
      path: string,
    ): Promise<FileDiff> =>
      Promise.resolve(
        fileDiff({ ...file, path }, [hunk(0, 1, lines.map(add))], {
          source: "recorded",
        }),
      ),
  };
});

const mark = {
  reviewer: "tanim",
  reason: "schema work for a junior task",
  set_by: "backend",
};

/** The project as the fake daemon holds it between requests. */
let proposals: Map<string, ProposalDetail>;
let comments: Map<string, ReviewComment[]>;
let revisions: Revision[];
let nextComment: number;
/** While set, every write waits for it, so a test can see the write pending. */
let held: Promise<void> | null;

function hold(): () => void {
  let release: () => void = () => undefined;
  held = new Promise<void>((resolve) => {
    release = resolve;
  });
  return () => {
    held = null;
    release();
  };
}

function put(detail: ProposalDetail): void {
  proposals.set(detail.summary.id, detail);
}

function update(id: string, change: (p: ProposalDetail) => ProposalDetail) {
  const found = proposals.get(id);
  if (!found) throw new Error(`no ${id}`);
  put(change(found));
}

function seed(): void {
  nextComment = 10;
  held = null;
  diffReads.refs = [];
  revisions = [revision(8, "P-5"), revision(7, "P-2"), revision(1)];
  proposals = new Map();
  comments = new Map();
  put(
    proposalDetail(
      proposalSummary("P-4", {
        title: "Database schema for reports",
        current_version: 2,
        human_review: mark,
        latest_verdict: "approve",
        review_claim: "review/quality",
        updated_at: 5,
      }),
      {
        versions: [
          proposalVersion(1, {
            base: "r7",
            changeset: "C-11",
            evidence: "$ cargo nextest run -p db\n    46 passed, 0 failed",
          }),
          proposalVersion(2, {
            base: "r8",
            changeset: "C-18",
            evidence:
              "$ cargo nextest run -p db\n    48 passed, 0 failed for #T-9 in @web/report.tsx",
          }),
        ],
        reviews: [
          review(1, "review/security", "request_changes", {
            body: "missing index",
          }),
          review(2, "review/quality", "approve"),
        ],
        review_claim: {
          holder: "review/quality",
          claimed_at: 1,
          renewed_at: 2,
          stale: false,
        },
      },
    ),
  );
  comments.set("P-4", [
    reviewComment(1, 1, {
      author: "review/security",
      verdict: "request_changes",
      path: SCHEMA,
      start_line: 3,
      end_line: 3,
      body: "Add an index on (owner_id, report_date).",
      outdated: true,
    }),
    reviewComment(2, 2, {
      author: "backend/db",
      path: SCHEMA,
      start_line: 3,
      end_line: 3,
      reply_to: "rc-1",
      body: "Added reports_owner_date_idx in v2.",
    }),
  ]);
  put(
    proposalDetail(
      proposalSummary("P-3", {
        title: "Login form styles",
        author: "frontend/styles",
        turn: "with_author",
        latest_verdict: "request_changes",
        updated_at: 3,
      }),
      {
        reviews: [
          review(1, "review/quality", "request_changes", {
            body: "contrast too low",
          }),
        ],
      },
    ),
  );
  put(
    proposalDetail(
      proposalSummary("P-6", {
        title: "Charts",
        author: "frontend/charts",
        conflicted: true,
        turn: "with_author",
        review_claim: "review/quality",
        updated_at: 9,
      }),
      { conflict: conflictReport("P-6", 1) },
    ),
  );
  put(
    proposalDetail(
      proposalSummary("P-5", {
        title: "Report totals",
        state: "integrated",
        turn: null,
        updated_at: 8,
      }),
    ),
  );
  put(
    proposalDetail(
      proposalSummary("P-8", {
        title: "Old idea",
        state: "closed",
        turn: null,
        close_reason: "superseded",
      }),
    ),
  );
}

function summaries(slug: string): ProposalSummary[] {
  return slug === "demo" ? [...proposals.values()].map((p) => p.summary) : [];
}

function routes(): void {
  daemon.shell(["demo", "ledger"]);
  daemon.on("GET /api/v1/projects/:slug/proposals", (call) =>
    ok({ proposals: summaries(call.params.slug ?? "") }),
  );
  daemon.on("GET /api/v1/projects/:slug/proposals/:id", (call) => {
    const found = proposals.get(call.params.id ?? "");
    return found ? ok(found) : refuse(404, "proposal_not_found");
  });
  daemon.on("GET /api/v1/projects/:slug/proposals/:id/comments", (call) =>
    ok({ comments: comments.get(call.params.id ?? "") ?? [] }),
  );
  daemon.on("GET /api/v1/projects/:slug/revisions", () =>
    ok({ revisions, before: null }),
  );
  daemon.on(
    "POST /api/v1/projects/:slug/proposals/:id/comments",
    async (call) => {
      if (held) await held;
      const id = call.params.id ?? "";
      const body = call.body as Omit<ReviewComment, "id">;
      const current = proposals.get(id)?.summary.current_version ?? 1;
      nextComment += 1;
      const stored = reviewComment(nextComment, current, {
        ...body,
        author: "human:tanim",
        created_at: Date.now(),
      });
      comments.set(id, [...(comments.get(id) ?? []), stored]);
      return ok(stored, 201);
    },
  );
  daemon.on(
    "POST /api/v1/projects/:slug/proposals/:id/reviews",
    async (call) => {
      if (held) await held;
      const id = call.params.id ?? "";
      const body = call.body as {
        verdict: "approve" | "request_changes";
        body: string;
      };
      update(id, (p) => ({
        ...p,
        summary: {
          ...p.summary,
          turn:
            body.verdict === "request_changes" ? "with_author" : p.summary.turn,
          latest_verdict: body.verdict,
          human_approved: body.verdict === "approve",
        },
        reviews: [
          ...p.reviews,
          review(p.summary.current_version, "human:tanim", body.verdict, {
            body: body.body,
            created_at: Date.now(),
          }),
        ],
      }));
      const p = proposals.get(id);
      return ok(
        {
          proposal: id,
          version: p?.summary.current_version,
          verdict: body.verdict,
          comments: [],
          summary: p?.summary,
        },
        201,
      );
    },
  );
}

/** Integrates as the daemon does: the proposal is integrated and main gains a revision that names it. */
function integrate(id: string, number: number): void {
  update(id, (p) => ({
    ...p,
    summary: {
      ...p.summary,
      state: "integrated",
      turn: null,
      closed_at: Date.now(),
    },
  }));
  revisions = [revision(number, id), ...revisions];
}

beforeEach(() => {
  seed();
  routes();
  vi.spyOn(window, "scrollTo").mockImplementation(() => undefined);
});
afterEach(() => {
  daemon.reset();
  cleanup();
  vi.restoreAllMocks();
});

function renderAt(path: string) {
  const queryClient = createQueryClient();
  const router = createAppRouter(queryClient);
  router.update({
    history: createMemoryHistory({ initialEntries: [path] }),
    context: { queryClient },
  });
  const view = render(
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </ThemeProvider>,
  );
  return { router, queryClient, container: view.container };
}

/** A stored event as the stream delivers it. */
function deliver(
  queryClient: QueryClient,
  type: string,
  fields: Record<string, unknown>,
): void {
  const frame: unknown = {
    kind: "event",
    seq: 100,
    type,
    actor: "review/quality",
    task: null,
    proposal: null,
    workspace: null,
    revision: null,
    data: {},
    created_at: Date.now(),
    ...fields,
  };
  if (!isEventEnvelope(frame)) throw new Error("not a frame");
  act(() => {
    applyEvent(queryClient, "demo", frame);
  });
}

function table() {
  return screen.findByRole("table");
}

function rowIds(element: HTMLElement): string[] {
  return within(element)
    .getAllByRole("row")
    .slice(1)
    .map((row) => row.querySelector("td")?.textContent ?? "");
}

async function integration() {
  return screen.findByRole("region", { name: "Integration" });
}

async function lineCell(container: HTMLElement, key: string) {
  await waitFor(() => {
    expect(container.querySelector(`[data-line="${key}"]`)).not.toBeNull();
  });
  const cell = container.querySelector(`[data-line="${key}"]`);
  if (!(cell instanceof HTMLElement)) throw new Error(`no line ${key}`);
  return cell;
}

function writes(route: string): Call[] {
  return daemon.made(route);
}

describe("Proposals list", () => {
  it("shows the open proposals with their task, author, version, waiting party, review state and claim", async () => {
    renderAt("/p/demo/proposals");
    const rows = await table();
    expect(rowIds(rows)).toEqual(["P-6", "P-4", "P-3"]);
    const p4 = within(rows)
      .getByRole("link", { name: "Database schema for reports" })
      .closest("tr");
    if (!p4) throw new Error("no P-4 row");
    const cells = within(p4);
    expect(cells.getByText("T-3")).toBeTruthy();
    expect(cells.getByText("@backend/db")).toBeTruthy();
    expect(cells.getByText("v2 of 2")).toBeTruthy();
    expect(cells.getAllByText("@tanim")).toHaveLength(3);
    expect(
      cells.getByText(byContent("Agent approved · needs @tanim")),
    ).toBeTruthy();
    expect(
      cells.getByText(byContent("claimed by @review/quality")),
    ).toBeTruthy();
    expect(cells.getByText("Human review").textContent).toContain("tanim");
    const p3 = within(rows)
      .getByRole("link", { name: "Login form styles" })
      .closest("tr");
    if (!p3) throw new Error("no P-3 row");
    expect(within(p3).getByText("Changes requested")).toBeTruthy();
    expect(within(p3).getAllByText("@frontend/styles")).toHaveLength(2);
    expect(
      screen
        .getByRole("link", { name: "Open · 3" })
        .getAttribute("aria-current"),
    ).toBe("page");
  });

  it("shows integrated proposals with the revision main names them in, and closed ones", async () => {
    const user = userEvent.setup();
    renderAt("/p/demo/proposals");
    await table();
    await user.click(screen.getByRole("link", { name: "Integrated" }));
    await waitFor(async () => {
      expect(rowIds(await table())).toEqual(["P-5"]);
    });
    expect(await screen.findByText("Integrated as r8")).toBeTruthy();
    await user.click(screen.getByRole("link", { name: "Closed" }));
    await waitFor(async () => {
      expect(rowIds(await table())).toEqual(["P-8"]);
    });
  });

  it("filters by claim and by conflict, and the address carries the filters", async () => {
    const user = userEvent.setup();
    const { router } = renderAt("/p/demo/proposals");
    await table();
    await user.click(screen.getByRole("button", { name: "Any claim" }));
    await user.click(
      await screen.findByRole("menuitem", { name: "Unclaimed" }),
    );
    await waitFor(async () => {
      expect(rowIds(await table())).toEqual(["P-3"]);
    });
    expect(router.state.location.search).toEqual({ claim: "unclaimed" });
    await user.click(screen.getByRole("button", { name: "Unclaimed" }));
    await user.click(await screen.findByRole("menuitem", { name: "Claimed" }));
    await user.click(screen.getByRole("button", { name: "Conflicted" }));
    await waitFor(async () => {
      expect(rowIds(await table())).toEqual(["P-6"]);
    });
  });

  it("shows a proposal an agent opens without a reload", async () => {
    const { queryClient } = renderAt("/p/demo/proposals");
    await table();
    put(
      proposalDetail(
        proposalSummary("P-9", {
          title: "Export CSV",
          task: "T-9",
          updated_at: 20,
        }),
      ),
    );
    deliver(queryClient, "proposal.opened", { proposal: "P-9", task: "T-9" });
    await waitFor(async () => {
      expect(rowIds(await table())).toEqual(["P-9", "P-6", "P-4", "P-3"]);
    });
  });

  it("shows the empty state for a project without proposals", async () => {
    renderAt("/p/ledger/proposals");
    expect(
      await screen.findByRole("region", { name: "No proposals yet" }),
    ).toBeTruthy();
  });
});

describe("Proposal detail", () => {
  it("compares each version with its base and two versions by their changesets; a v1 comment is outdated on v2 and current on v1", async () => {
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4");
    const thread = await screen.findByRole("list", { name: "Thread rc-1" });
    expect(within(thread).getByText("on v1 · line 3 · outdated")).toBeTruthy();
    expect(within(thread).getByText("reply · v2")).toBeTruthy();
    expect(diffReads.refs).toContain("r8..C-18");
    expect(
      screen
        .getByRole("link", { name: "v2 · approved" })
        .getAttribute("aria-current"),
    ).toBe("page");
    expect(await screen.findByText("base r8 → main r8")).toBeTruthy();

    await user.click(
      screen.getByRole("link", { name: "v1 · changes requested" }),
    );
    expect(await screen.findByText("on v1 · line 3")).toBeTruthy();
    expect(diffReads.refs).toContain("r7..C-11");
    expect(screen.queryByText(/outdated/)).toBeNull();
    expect(screen.getByText(/v1 is an earlier version/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Approve v2" })).toBeNull();
    expect(screen.getByText(/46 passed/)).toBeTruthy();

    await user.click(screen.getByRole("link", { name: "Compare v1 → v2" }));
    await waitFor(() => {
      expect(diffReads.refs).toContain("C-11..C-18");
    });
    await waitFor(() => {
      expect(screen.queryByRole("list", { name: "Thread rc-1" })).toBeNull();
    });
  });

  it("posts a comment on one line and on a run of lines with the range the selection gives, showing it after the server answers", async () => {
    const user = userEvent.setup();
    const { container } = renderAt("/p/demo/proposals/P-4");
    await user.click(await lineCell(container, "new:4"));
    let form = await screen.findByRole("form", {
      name: "Comment on line 4 · new side · tanim",
    });
    await user.type(within(form).getByLabelText("Comment"), "Needs NOT NULL.");
    await user.click(
      within(form).getByRole("button", { name: "Add to review" }),
    );
    expect(await screen.findByText("Needs NOT NULL.")).toBeTruthy();
    expect(
      writes("POST /api/v1/projects/demo/proposals/P-4/comments")[0]?.body,
    ).toEqual({
      path: SCHEMA,
      side: "new",
      start_line: 4,
      end_line: 4,
      body: "Needs NOT NULL.",
      reply_to: null,
    });

    await user.click(await lineCell(container, "new:2"));
    fireEvent.click(await lineCell(container, "new:4"), { shiftKey: true });
    form = await screen.findByRole("form", {
      name: "Comment on lines 2–4 · new side · tanim",
    });
    await user.type(
      within(form).getByLabelText("Comment"),
      "owner_id and report_date need defaults.",
    );
    const release = hold();
    await user.click(
      within(form).getByRole("button", { name: "Add to review" }),
    );
    expect(
      within(form)
        .getByRole("button", { name: "Add to review" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(
      screen.queryByText("owner_id and report_date need defaults.", {
        selector: "p",
      }),
    ).toBeNull();
    release();
    expect(
      await screen.findByText("owner_id and report_date need defaults.", {
        selector: "p",
      }),
    ).toBeTruthy();
    expect(screen.getByText("on v2 · lines 2–4")).toBeTruthy();
    expect(
      writes("POST /api/v1/projects/demo/proposals/P-4/comments")[1]?.body,
    ).toMatchObject({ start_line: 2, end_line: 4, side: "new" });
  });

  it("suggests items and files in the line comment and the request-changes box", async () => {
    mentionRoutes();
    const user = userEvent.setup();
    const { container } = renderAt("/p/demo/proposals/P-4");
    await user.click(await lineCell(container, "new:4"));
    const form = await screen.findByRole("form", {
      name: "Comment on line 4 · new side · tanim",
    });
    await expectMentionPopover(within(form).getByLabelText("Comment"));
    const panel = await integration();
    await user.click(
      within(panel).getByRole("button", { name: "Request changes" }),
    );
    await expectMentionPopover(
      within(panel).getByLabelText("What needs to change"),
    );
  });

  it("links the mentions in a comment thread and in a review", async () => {
    comments.set("P-4", [
      reviewComment(1, 2, {
        author: "backend/db",
        path: SCHEMA,
        start_line: 3,
        end_line: 3,
        body: "Context in #T-9, code in @web/report.tsx.",
      }),
    ]);
    const user = userEvent.setup();
    const { container } = renderAt("/p/demo/proposals/P-4");
    const thread = await screen.findByRole("list", { name: "Thread rc-1" });
    expect(
      within(thread).getByRole("link", { name: "T-9" }).getAttribute("href"),
    ).toBe("/p/demo/t/T-9");
    expect(
      within(thread)
        .getByRole("link", { name: "web/report.tsx" })
        .getAttribute("href"),
    ).toBe("/p/demo/repository?path=web%2Freport.tsx");
    expect(container).toBeTruthy();
    const evidence = screen.getByRole("region", {
      name: "Evidence from the author",
    });
    expect(
      within(evidence).getByRole("link", { name: "T-9" }).getAttribute("href"),
    ).toBe("/p/demo/t/T-9");
    expect(
      within(evidence)
        .getByRole("link", { name: "web/report.tsx" })
        .getAttribute("href"),
    ).toBe("/p/demo/repository?path=web%2Freport.tsx");
    const panel = await integration();
    await user.click(
      within(panel).getByRole("button", { name: "Request changes" }),
    );
    await user.type(
      within(panel).getByLabelText("What needs to change"),
      "Fix #T-9 first",
    );
    await user.click(
      within(panel).getByRole("button", { name: "Send request" }),
    );
    const reviews = screen.getByRole("region", { name: "Reviews" });
    expect(
      await within(reviews).findByRole("link", { name: "T-9" }),
    ).toBeTruthy();
  });

  it("shows a comment refusal on the form", async () => {
    daemon.on(
      "POST /api/v1/projects/:slug/proposals/:id/comments",
      refuse(422, "invalid_comment_range"),
    );
    const user = userEvent.setup();
    const { container } = renderAt("/p/demo/proposals/P-4");
    await user.click(await lineCell(container, "new:3"));
    const form = await screen.findByRole("form", { name: /Comment on line 3/ });
    await user.type(within(form).getByLabelText("Comment"), "x");
    await user.click(
      within(form).getByRole("button", { name: "Add to review" }),
    );
    expect(
      await within(form).findByText(
        "The comment was not saved. These lines are not in the file on that side.",
      ),
    ).toBeTruthy();
  });

  it("shows Approve as pending, then the human approval once the server answers", async () => {
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4");
    const panel = await integration();
    expect(
      within(panel).getByText(byContent(/Waiting for @tanim/)),
    ).toBeTruthy();
    const reviews = screen.getByRole("region", { name: "Reviews" });
    const before = within(reviews).getAllByRole("listitem").length;
    const release = hold();
    await user.click(within(panel).getByRole("button", { name: "Approve v2" }));
    expect(
      within(panel)
        .getByRole("button", { name: "Approve v2" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(within(reviews).getAllByRole("listitem")).toHaveLength(before);
    release();
    await waitFor(() => {
      expect(within(reviews).getAllByRole("listitem")).toHaveLength(before + 1);
    });
    expect(
      within(reviews).getByText("@tanim").closest("p")?.textContent,
    ).toMatch(/^@tanim · v2/);
    expect(
      within(panel).getByText(byContent(/Approved by @tanim/)),
    ).toBeTruthy();
    expect(
      writes("POST /api/v1/projects/demo/proposals/P-4/reviews")[0]?.body,
    ).toEqual({ verdict: "approve", body: "approved in the web interface" });
  });

  it("needs a body for Request changes, shows it pending, then the verdict once the server answers", async () => {
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4");
    const panel = await integration();
    await user.click(
      within(panel).getByRole("button", { name: "Request changes" }),
    );
    const form = within(panel).getByRole("form", { name: "Request changes" });
    await user.click(
      within(form).getByRole("button", { name: "Send request" }),
    );
    expect(
      await within(form).findByText("Say what needs to change."),
    ).toBeTruthy();
    await user.type(
      within(form).getByLabelText("What needs to change"),
      "Rename the index",
    );
    const release = hold();
    await user.click(
      within(form).getByRole("button", { name: "Send request" }),
    );
    expect(
      within(form)
        .getByRole("button", { name: "Send request" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    const reviews = screen.getByRole("region", { name: "Reviews" });
    release();
    expect(
      await within(reviews).findByText("Requested changes: Rename the index"),
    ).toBeTruthy();
    expect(
      screen.getByText("Changes requested · back with the author"),
    ).toBeTruthy();
  });

  it("integrates a proposal whose version had a request for changes: pending, then the result and the new revision, and no Reject", async () => {
    daemon.on(
      "POST /api/v1/projects/:slug/proposals/:id/integrate",
      async (call) => {
        if (held) await held;
        integrate("P-3", 9);
        return ok({
          proposal: call.params.id,
          result: "succeeded",
          revision: "r9",
        });
      },
    );
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-3");
    const panel = await integration();
    expect(
      screen.getByText("Changes requested · back with the author"),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: /reject/i })).toBeNull();
    const release = hold();
    await user.click(within(panel).getByRole("button", { name: "Integrate" }));
    expect(
      within(panel)
        .getByRole("button", { name: "Integrate" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(within(panel).queryByText(/Integrated as/)).toBeNull();
    release();
    const link = await within(panel).findByRole("link", { name: "r9" });
    expect(link.getAttribute("href")).toBe("/p/demo/repository?revision=r9");
    expect(screen.getByText("Integrated as r9")).toBeTruthy();
    expect(
      within(panel).queryByRole("button", { name: "Integrate" }),
    ).toBeNull();
    expect(
      writes("POST /api/v1/projects/demo/proposals/P-3/integrate")[0]?.body,
    ).toEqual({ expected_version: 1 });
  });

  it("shows stale_version on the action and reloads the proposal to its newer version", async () => {
    daemon.on("POST /api/v1/projects/:slug/proposals/:id/integrate", () => {
      update("P-4", (p) => ({
        ...p,
        summary: { ...p.summary, current_version: 3 },
        versions: [
          ...p.versions,
          proposalVersion(3, { base: "r8", changeset: "C-22" }),
        ],
      }));
      return refuse(409, "stale_version", {
        expected_version: 2,
        current_version: 3,
      });
    });
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4");
    const panel = await integration();
    await user.click(within(panel).getByRole("button", { name: "Integrate" }));
    expect(
      await within(panel).findByText(
        "v3 is now the current version. Read it, then try once more.",
      ),
    ).toBeTruthy();
    expect(
      await screen.findByRole("link", {
        name: "v3 · no verdict yet",
        current: "page",
      }),
    ).toBeTruthy();
    expect(
      within(panel).getByRole("button", { name: "Approve v3" }),
    ).toBeTruthy();
    expect(screen.queryByText(/The request failed/)).toBeNull();
  });

  it("shows an unmapped error code in the generic banner with the server's message", async () => {
    daemon.on(
      "POST /api/v1/projects/:slug/proposals/:id/reviews",
      refuse(500, "disk_full"),
    );
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4");
    const panel = await integration();
    await user.click(within(panel).getByRole("button", { name: "Approve v2" }));
    const banner = await screen.findByText("refused: disk_full");
    expect(banner.closest("[role=alert]")?.textContent).toMatch(
      /The request failed/,
    );
    expect(within(panel).queryByRole("alert")).toBeNull();
  });

  it("shows the result and the new revision after an agent integrates", async () => {
    const { queryClient } = renderAt("/p/demo/proposals/P-4");
    const panel = await integration();
    integrate("P-4", 9);
    deliver(queryClient, "proposal.integrated", {
      proposal: "P-4",
      task: "T-3",
      revision: "r9",
      data: { version: 2 },
    });
    deliver(queryClient, "main.advanced", { revision: "r9" });
    const link = await within(panel).findByRole("link", { name: "r9" });
    expect(link.getAttribute("href")).toBe("/p/demo/repository?revision=r9");
    expect(within(panel).getByText(/^v2 · /)).toBeTruthy();
    expect(
      within(panel).queryByRole("button", { name: "Approve v2" }),
    ).toBeNull();
  });

  it("switches to the conflict report when a conflict arrives, with no reload", async () => {
    const { queryClient } = renderAt("/p/demo/proposals/P-4");
    await integration();
    expect(
      screen.queryByRole("region", { name: "Conflict report" }),
    ).toBeNull();
    update("P-4", (p) => ({
      ...p,
      summary: { ...p.summary, conflicted: true, turn: "with_author" },
      conflict: conflictReport("P-4", 2),
    }));
    deliver(queryClient, "integration.conflicted", {
      proposal: "P-4",
      task: "T-3",
      revision: "r8",
    });
    const report = await screen.findByRole("region", {
      name: "Conflict report",
    });
    const kinds = [...report.querySelectorAll("[data-kind]")].map((el) =>
      el.getAttribute("data-kind"),
    );
    expect(kinds.sort()).toEqual([
      "add_add",
      "both_modified_binary",
      "modify_delete",
      "text",
      "type_change",
    ]);
    expect(within(report).getAllByText(/^tree [0-9a-f]{7}$/)).toHaveLength(3);
    expect(
      within(report).getByText("const max = Math.max(...totals, 1);"),
    ).toBeTruthy();
    expect(within(report).getByText("P-4 v2 · after line 3")).toBeTruthy();
    expect(
      within(report).getByText(byContent(/@backend\/db runs sync_workspace/)),
    ).toBeTruthy();
    expect(
      screen.getByText("Conflict with main r8 · back with the author"),
    ).toBeTruthy();
    const panel = await integration();
    expect(
      within(panel).getByText("Does not merge onto r8: 5 files conflict"),
    ).toBeTruthy();
  });

  it("shows each gate's state as the daemon reports it", async () => {
    put(
      proposalDetail(proposalSummary("P-7", { title: "Gated" }), {
        gates: humanGates({
          prerequisites: { status: "unmet", code: "prerequisites_not_done" },
        }),
        prerequisites: [
          {
            task: "T-1",
            title: "T-1",
            description: "",
            status: "in_progress",
          },
        ],
      }),
    );
    renderAt("/p/demo/proposals/P-7");
    const panel = await integration();
    expect(within(panel).getAllByRole("img", { name: "met" })).toHaveLength(3);
    expect(within(panel).getByRole("img", { name: "not met" })).toBeTruthy();
    expect(
      within(panel).getByRole("img", { name: "checked at integration" }),
    ).toBeTruthy();
    expect(within(panel).getByText("Prerequisites not Done: T-1")).toBeTruthy();
    const integrate = within(panel).getByRole("button", { name: "Integrate" });
    expect(integrate.hasAttribute("disabled")).toBe(true);
    expect(integrate.getAttribute("aria-describedby")).toBeTruthy();
    expect(
      within(panel).getByText(
        "Prerequisites not Done: T-1, so you cannot integrate yet.",
      ),
    ).toBeTruthy();
    expect(
      within(panel)
        .getByRole("button", { name: "Approve v1" })
        .hasAttribute("disabled"),
    ).toBe(false);
  });

  it("disables Integrate with the reason when the mark waits for another human", async () => {
    put(
      proposalDetail(
        proposalSummary("P-8", {
          title: "Marked for ana",
          latest_verdict: "approve",
          human_review: { reviewer: "ana", reason: "", set_by: "backend" },
        }),
        {
          gates: humanGates({
            human_review: { status: "unmet", code: "human_approval_missing" },
          }),
        },
      ),
    );
    renderAt("/p/demo/proposals/P-8");
    const panel = await integration();
    expect(
      within(panel)
        .getByRole("button", { name: "Integrate" })
        .hasAttribute("disabled"),
    ).toBe(true);
    expect(
      within(panel).getByText("Integrate waits for @ana to approve v1."),
    ).toBeTruthy();
  });

  it("says when the project has no such proposal", async () => {
    renderAt("/p/demo/proposals/P-99");
    expect(
      await screen.findByRole("region", { name: "No proposal P-99" }),
    ).toBeTruthy();
  });
});
