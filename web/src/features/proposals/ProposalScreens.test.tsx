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
import type { ThreadView } from "@/api/messages";
import type {
  ChangeRange,
  NewQuestion,
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
import { message, recipient, thread } from "@/test/messages";
import {
  checkRun,
  conflictReport,
  humanGates,
  intent,
  proposalDetail,
  proposalSummary,
  proposalVersion,
  review,
  reviewComment,
  revision,
} from "@/test/proposals";
import { requestDetail, requestSummary } from "@/test/requests";
import { ThemeProvider } from "@/theme/ThemeProvider";
import { needsItem } from "@/test/workers";
import type { RequestCode, RequestSummary } from "@/api/requests";

vi.mock("@/api/client", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/api/client")>();
  const { daemon: fake } = await import("@/test/daemon");
  return { ...real, api: fake.api };
});

const SCHEMA = "migrations/0004_reports.sql";

/** Writes an intent of one part named Reports table into the edit's intent form, whose part holds the edited file. */
function writeIntent(form: HTMLElement, summary: string): void {
  fireEvent.change(within(form).getByLabelText("Summary"), {
    target: { value: summary },
  });
  fireEvent.change(within(form).getByLabelText("Part 1 name"), {
    target: { value: "Reports table" },
  });
  fireEvent.change(within(form).getByLabelText("What part 1 does"), {
    target: { value: "Adds a check note to the table." },
  });
}

// The version diff is the proposal diff seam's to read; the proposal screens only name the range they read.
const diffReads = vi.hoisted(() => ({
  refs: [] as string[],
  /** The schema file is a change of line 20 of a 30-line file, not a new file, so unchanged lines lie around its hunk. */
  modified: false,
}));
vi.mock("@/api/proposals", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/proposals")>();
  const { add, comparedFile, comparison, ctx, del, fileDiff, hunk, state } =
    await import("@/test/diff");
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
  const modified = comparedFile("migrations/0004_reports.sql", "modified", {
    added_lines: 1,
    removed_lines: 1,
    old_hash: "e".repeat(64),
    new_hash: "f".repeat(64),
  });
  const shown = () => (diffReads.modified ? modified : file);
  const from = (range: ChangeRange) =>
    range.since === null ? "base" : `v${String(range.since)}`;
  const around = (from: number, to: number) =>
    Array.from({ length: to - from + 1 }, (_, i) =>
      ctx(`-- ${String(from + i)}`),
    );
  return {
    ...actual,
    getProposalChanges: (
      _slug: string,
      _id: string,
      range: ChangeRange,
    ): Promise<Comparison> => {
      diffReads.refs.push(`${from(range)}..v${String(range.version)}`);
      return Promise.resolve(
        comparison([shown()], {
          source: "recorded",
          from: state("changeset", from(range)),
          to: state("changeset", `v${String(range.version)}`),
        }),
      );
    },
    getProposalFileDiff: (
      _slug: string,
      _id: string,
      _range: ChangeRange,
      path: string,
    ): Promise<FileDiff> =>
      Promise.resolve(
        diffReads.modified
          ? fileDiff(
              { ...modified, path },
              [
                hunk(17, 17, [
                  ...around(17, 19),
                  del("-- 20"),
                  add("-- twenty"),
                  ...around(21, 23),
                ]),
              ],
              { source: "recorded" },
            )
          : fileDiff({ ...file, path }, [hunk(0, 1, lines.map(add))], {
              source: "recorded",
            }),
      ),
  };
});

/** The new side of the modified schema file: 30 lines, line 20 changed. */
const MODIFIED_TEXT = Array.from({ length: 30 }, (_, i) =>
  i === 19 ? "-- twenty\n" : `-- ${String(i + 1)}\n`,
).join("");

const mark = {
  reviewer: "tanim",
  reason: "schema work for a junior task",
  set_by: "backend",
};

/** The project as the fake daemon holds it between requests. */
let proposals: Map<string, ProposalDetail>;
let comments: Map<string, ReviewComment[]>;
/** The threads of questions to authors, by proposal. */
let questions: Map<string, ThreadView[]>;
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
  diffReads.modified = false;
  revisions = [revision(8, "P-5"), revision(7, "P-2"), revision(1)];
  proposals = new Map();
  comments = new Map();
  questions = new Map();
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
        agent_approved: true,
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
    ok({
      proposals: summaries(call.params.slug ?? ""),
      waiting_for_you: call.params.slug === "demo" ? ["P-4"] : [],
    }),
  );
  daemon.on("GET /api/v1/projects/:slug/needs-you", (call) =>
    ok({
      items:
        call.params.slug === "demo"
          ? [
              needsItem({
                kind: "human_review",
                screen: "proposal",
                proposal: "P-4",
                human: "tanim",
              }),
              needsItem({
                kind: "human_review",
                screen: "proposal",
                proposal: "P-5",
                human: "ana",
              }),
            ]
          : [],
    }),
  );
  daemon.on("GET /api/v1/projects/:slug/proposals/:id", (call) => {
    const found = proposals.get(call.params.id ?? "");
    if (!found) return refuse(404, "proposal_not_found");
    // An earlier version's read carries that version's check runs, which these tests leave empty.
    const asked = new URLSearchParams(call.path.split("?")[1] ?? "").get(
      "version",
    );
    return asked === null || Number(asked) === found.summary.current_version
      ? ok(found)
      : ok({ ...found, checks: [] });
  });
  daemon.on("GET /api/v1/projects/:slug/proposals/:id/file", (call) => {
    const query = new URLSearchParams(call.path.split("?")[1] ?? "");
    return ok({
      path: query.get("path"),
      kind: "file",
      hash: "f".repeat(64),
      size: MODIFIED_TEXT.length,
      binary: false,
      too_large: false,
      text: MODIFIED_TEXT,
    });
  });
  daemon.on("GET /api/v1/projects/:slug/proposals/:id/comments", (call) =>
    ok({ comments: comments.get(call.params.id ?? "") ?? [] }),
  );
  daemon.on("GET /api/v1/projects/:slug/messages", (call) => {
    const query = new URLSearchParams(call.path.split("?")[1] ?? "");
    const items = questions.get(query.get("proposal") ?? "") ?? [];
    return ok({ items, next: null, total_awaiting_human: 0 });
  });
  daemon.on(
    "POST /api/v1/projects/:slug/proposals/:id/questions",
    async (call) => {
      if (held) await held;
      const id = call.params.id ?? "";
      const body = call.body as NewQuestion;
      const list = questions.get(id) ?? [];
      const number = `m-${String(40 + list.length)}`;
      const stored = message(number, {
        sender: "human:tanim",
        recipients: [recipient("backend/db", { addressed_as: "worker" })],
        body: body.body,
        task: "T-4",
        proposal: id,
        lines:
          body.path == null ||
          body.side == null ||
          body.start_line == null ||
          body.end_line == null
            ? null
            : {
                version: body.version,
                path: body.path,
                side: body.side,
                start_line: body.start_line,
                end_line: body.end_line,
                excerpt: "x",
                excerpt_cut: false,
              },
      });
      questions.set(id, [
        ...list,
        thread(number, { proposal: id, messages: [stored] }),
      ]);
      return ok(stored, 201);
    },
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
    .map((row) => row.querySelector("td span span")?.textContent ?? "");
}

async function integration() {
  return screen.findByRole("region", { name: "Integration" });
}

/** The human's decisions, in the bar with the tabs. */
async function decisions() {
  return screen.findByRole("group", { name: "Your review" });
}

async function lineCell(container: HTMLElement, key: string) {
  await waitFor(() => {
    expect(container.querySelector(`[data-line="${key}"]`)).not.toBeNull();
  });
  const cell = container.querySelector(`[data-line="${key}"]`);
  if (!(cell instanceof HTMLElement)) throw new Error(`no line ${key}`);
  return cell;
}

/** Picks an action of the menu at the selected lines. */
async function choose(
  user: ReturnType<typeof userEvent.setup>,
  action: "Ask the author" | "Review comment" | "Make a request",
): Promise<void> {
  const menu = await screen.findByRole("group", { name: "Selected lines" });
  await user.click(within(menu).getByRole("button", { name: action }));
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
    expect(cells.getByRole("link", { name: "T-3" }).getAttribute("href")).toBe(
      "/p/demo/t/T-3",
    );
    expect(cells.getByText("@backend/db")).toBeTruthy();
    expect(
      cells.getByText(
        byContent(
          "for T-3 · v2 of 2 · claimed by @review/quality · human review by @tanim",
        ),
      ),
    ).toBeTruthy();
    expect(cells.getAllByText("@tanim")).toHaveLength(1);
    expect(
      cells.getByText(byContent("Agent approved · waits for you")),
    ).toBeTruthy();
    const p3 = within(rows)
      .getByRole("link", { name: "Login form styles" })
      .closest("tr");
    if (!p3) throw new Error("no P-3 row");
    expect(
      within(p3).getByText(
        byContent("Changes requested · waits for @frontend/styles"),
      ),
    ).toBeTruthy();
    expect(within(p3).getAllByText("@frontend/styles")).toHaveLength(2);
    expect(
      screen.getByRole("link", { name: "Open 3" }).getAttribute("aria-current"),
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

  it("names the review batch a proposal integrated with and links it", async () => {
    revisions = [{ ...revision(8, "P-5"), batch: "rb-2" }, revision(1)];
    renderAt("/p/demo/proposals/P-5");
    expect(
      await screen.findByText("Integrated with batch rb-2 as r8"),
    ).toBeTruthy();
    const panel = await integration();
    expect(
      await within(panel).findByText(
        byContent(/^v1 integrated with batch rb-2 as r8/),
      ),
    ).toBeTruthy();
    const link = within(panel).getByRole("link", { name: "rb-2" });
    expect(link.getAttribute("href")).toBe("/p/demo/batches/rb-2");
  });

  it("filters by claim and by conflict, and the address carries the filters", async () => {
    const user = userEvent.setup();
    const { router } = renderAt("/p/demo/proposals");
    await table();
    await user.click(screen.getByRole("button", { name: "Filter" }));
    await user.click(
      await screen.findByRole("menuitemradio", { name: "Unclaimed" }),
    );
    await waitFor(async () => {
      expect(rowIds(await table())).toEqual(["P-3"]);
    });
    expect(router.state.location.search).toEqual({ claim: "unclaimed" });
    await user.click(
      await screen.findByRole("menuitemradio", { name: "Claimed" }),
    );
    await user.click(
      screen.getByRole("menuitemcheckbox", { name: /^Conflicted/ }),
    );
    await waitFor(async () => {
      expect(rowIds(await table())).toEqual(["P-6"]);
    });
    await user.keyboard("{Escape}");
    await user.click(
      screen.getByRole("button", { name: "Remove the filter Conflicted" }),
    );
    await waitFor(async () => {
      expect(rowIds(await table())).toEqual(["P-6", "P-4"]);
    });
    expect(router.state.location.search).toEqual({ claim: "claimed" });
  });

  it("marks the proposals that wait for your review, counts them on the tab and shows only them with the filter", async () => {
    const user = userEvent.setup();
    const { router } = renderAt("/p/demo/proposals");
    const rows = await table();
    const marked = within(rows)
      .getAllByText(byContent(/ · waits for you$/))
      .map(
        (mark) =>
          mark.closest("tr")?.querySelector("td span span")?.textContent,
      );
    expect(marked).toEqual(["P-4"]);
    const nav = screen.getByRole("navigation", { name: "Project" });
    expect(
      (
        await within(nav).findByRole("status", {
          name: "1 proposal waits for your review",
        })
      ).textContent,
    ).toBe("1");
    await user.click(screen.getByRole("button", { name: "Filter" }));
    await user.click(
      await screen.findByRole("menuitemcheckbox", { name: "Waits for you 1" }),
    );
    await waitFor(async () => {
      expect(rowIds(await table())).toEqual(["P-4"]);
    });
    expect(router.state.location.search).toEqual({ waiting: true });
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
    renderAt("/p/demo/proposals/P-4?tab=changes");
    const thread = await screen.findByRole("list", { name: "Thread rc-1" });
    expect(within(thread).getByText("on v1 · line 3 · outdated")).toBeTruthy();
    expect(within(thread).getByText("reply · v2")).toBeTruthy();
    expect(diffReads.refs).toContain("base..v2");
    expect(
      screen.getByRole("button", { name: "Version: v2 · approved" }),
    ).toBeTruthy();

    await user.click(
      screen.getByRole("button", { name: "Version: v2 · approved" }),
    );
    await user.click(
      await screen.findByRole("menuitem", { name: "v1 · changes requested" }),
    );
    expect(await screen.findByText("on v1 · line 3")).toBeTruthy();
    expect(diffReads.refs).toContain("base..v1");
    expect(screen.queryByText(/outdated/)).toBeNull();
    expect(screen.getByText(/v1 is an earlier version/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Approve v2" })).toBeNull();

    expect(screen.queryByRole("navigation", { name: "Change" })).toBeNull();
  });

  it("lists a comment on a line outside the shown hunks under Other comments", async () => {
    comments.set("P-4", [
      reviewComment(1, 2, {
        author: "review/security",
        path: SCHEMA,
        start_line: 40,
        end_line: 40,
        body: "This line is not in the diff.",
      }),
    ]);
    renderAt("/p/demo/proposals/P-4?tab=changes");
    const other = await screen.findByRole("region", { name: "Other comments" });
    expect(
      within(other).getByText("This line is not in the diff."),
    ).toBeTruthy();
  });

  it("switches to the change since the version the review asked changes on, where a new-side comment lands on the current version", async () => {
    const user = userEvent.setup();
    const { container } = renderAt("/p/demo/proposals/P-4?tab=changes");
    const change = await screen.findByRole("navigation", { name: "Change" });
    expect(
      within(change)
        .getByRole("link", { name: "Full change" })
        .getAttribute("aria-current"),
    ).toBe("page");
    await user.click(within(change).getByRole("link", { name: "Since v1" }));
    await waitFor(() => {
      expect(diffReads.refs).toContain("v1..v2");
    });
    expect(
      screen.getByRole("button", { name: "Version: v2 · approved" }),
    ).toBeTruthy();
    await user.click(await lineCell(container, "new:4"));
    await choose(user, "Review comment");
    const form = await screen.findByRole("form", {
      name: "Comment on line 4 · new side · tanim",
    });
    await user.type(within(form).getByLabelText("Comment"), "Since v1.");
    await user.click(
      within(form).getByRole("button", { name: "Add to review" }),
    );
    expect(await screen.findByText("Since v1.")).toBeTruthy();
    expect(
      writes("POST /api/v1/projects/demo/proposals/P-4/comments")[0]?.body,
    ).toMatchObject({
      version: 2,
      path: SCHEMA,
      side: "new",
      start_line: 4,
      end_line: 4,
    });
  });

  it("posts a comment on one line and on a run of lines with the range the selection gives, showing it after the server answers", async () => {
    const user = userEvent.setup();
    const { container } = renderAt("/p/demo/proposals/P-4?tab=changes");
    await user.click(await lineCell(container, "new:4"));
    await choose(user, "Review comment");
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
      version: 2,
      path: SCHEMA,
      side: "new",
      start_line: 4,
      end_line: 4,
      body: "Needs NOT NULL.",
      reply_to: null,
    });

    await user.click(await lineCell(container, "new:2"));
    fireEvent.click(await lineCell(container, "new:4"), { shiftKey: true });
    await choose(user, "Review comment");
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
        .getByRole("button", { name: "Adding to review…" })
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

  it("shows the unchanged lines around a change from the version's file, keeps a thread's lines open and comments on a shown line", async () => {
    diffReads.modified = true;
    const user = userEvent.setup();
    const { container } = renderAt("/p/demo/proposals/P-4?tab=changes");
    // The outdated thread on line 3 keeps lines 1-6 open; lines 7-16 and 24-30 are hidden.
    await user.click(
      await screen.findByRole("button", {
        name: "Show all 10 unchanged lines",
      }),
    );
    expect(
      daemon.made(
        `GET /api/v1/projects/demo/proposals/P-4/file?version=2&path=${encodeURIComponent(SCHEMA)}`,
      ),
    ).toHaveLength(1);
    expect(
      screen.getByText("Add an index on (owner_id, report_date)."),
    ).toBeTruthy();
    expect(screen.queryByRole("region", { name: "Other comments" })).toBeNull();
    await user.click(await lineCell(container, "new:12"));
    await choose(user, "Review comment");
    const form = await screen.findByRole("form", {
      name: "Comment on line 12 · new side · tanim",
    });
    await user.type(within(form).getByLabelText("Comment"), "Why twelve?");
    await user.click(
      within(form).getByRole("button", { name: "Add to review" }),
    );
    expect(
      await screen.findByText("Why twelve?", { selector: "p" }),
    ).toBeTruthy();
    expect(
      writes("POST /api/v1/projects/demo/proposals/P-4/comments")[0]?.body,
    ).toMatchObject({ version: 2, side: "new", start_line: 12, end_line: 12 });
    await user.click(screen.getByRole("button", { name: "Show whole file" }));
    await user.click(screen.getByRole("button", { name: "Show changes only" }));
    expect(screen.getByText("Why twelve?", { selector: "p" })).toBeTruthy();
    expect(container.querySelector('[data-line="new:12"]')).not.toBeNull();
    expect(container.querySelector('[data-line="new:8"]')).toBeNull();
    expect(screen.queryByRole("region", { name: "Other comments" })).toBeNull();
  });

  it("says at the selection why an older version takes no comment, and links to the current version", async () => {
    const user = userEvent.setup();
    const { container } = renderAt(
      "/p/demo/proposals/P-4?tab=changes&version=1",
    );
    await user.click(await lineCell(container, "new:3"));
    const note = await screen.findByText(
      /This is v1\. Comments go on the current version, v2\./,
    );
    expect(note.getAttribute("role")).toBe("status");
    expect(screen.queryByRole("form", { name: /Comment on line/ })).toBeNull();
    const link = screen.getByRole("link", { name: "Open v2" });
    expect(link.getAttribute("href")).toBe(
      `/p/demo/proposals/P-4?path=${encodeURIComponent(SCHEMA)}`,
    );
    fireEvent.click(await lineCell(container, "new:5"), { shiftKey: true });
    expect(screen.getByText(/This is v1\./)).toBeTruthy();
  });

  it("suggests items and files in the line comment and the request-changes box", async () => {
    mentionRoutes();
    const user = userEvent.setup();
    const { container } = renderAt("/p/demo/proposals/P-4?tab=changes");
    await user.click(await lineCell(container, "new:4"));
    await choose(user, "Review comment");
    const form = await screen.findByRole("form", {
      name: "Comment on line 4 · new side · tanim",
    });
    await expectMentionPopover(within(form).getByLabelText("Comment"));
    const panel = screen.getByRole("group", { name: "Your review" });
    await user.click(
      within(panel).getByRole("button", { name: "Request changes" }),
    );
    await expectMentionPopover(
      within(
        await screen.findByRole("form", { name: "Request changes" }),
      ).getByLabelText("What needs to change"),
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
    const { container } = renderAt("/p/demo/proposals/P-4?tab=changes");
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
    await user.click(screen.getByRole("tab", { name: "Overview" }));
    const evidence = await screen.findByRole("article", {
      name: "Version v2",
    });
    expect(
      within(evidence).getByRole("link", { name: "T-9" }).getAttribute("href"),
    ).toBe("/p/demo/t/T-9");
    expect(
      within(evidence)
        .getByRole("link", { name: "web/report.tsx" })
        .getAttribute("href"),
    ).toBe("/p/demo/repository?path=web%2Freport.tsx");
    const panel = await decisions();
    await user.click(
      within(panel).getByRole("button", { name: "Request changes" }),
    );
    const request = await screen.findByRole("form", {
      name: "Request changes",
    });
    await user.type(
      within(request).getByLabelText("What needs to change"),
      "Fix #T-9 first",
    );
    await user.click(
      within(request).getByRole("button", { name: "Send request" }),
    );
    const reviews = screen.getByRole("article", { name: "Version v2" });
    await waitFor(() => {
      expect(
        within(reviews).getAllByRole("link", { name: "T-9" }),
      ).toHaveLength(2);
    });
  });

  it("shows a comment refusal on the form", async () => {
    daemon.on(
      "POST /api/v1/projects/:slug/proposals/:id/comments",
      refuse(422, "invalid_comment_range"),
    );
    const user = userEvent.setup();
    const { container } = renderAt("/p/demo/proposals/P-4?tab=changes");
    await user.click(await lineCell(container, "new:3"));
    await choose(user, "Review comment");
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

  it("asks the author about selected lines, pending until the daemon answers, then marks the lines with a link to the thread and writes no review", async () => {
    const user = userEvent.setup();
    const { container } = renderAt("/p/demo/proposals/P-4?tab=changes");
    await user.click(await lineCell(container, "new:2"));
    fireEvent.click(await lineCell(container, "new:3"), { shiftKey: true });
    await choose(user, "Ask the author");
    const form = await screen.findByRole("form", {
      name: "Ask the author about lines 2–3 · new side · v2",
    });
    await user.click(
      within(form).getByRole("button", { name: "Send question" }),
    );
    expect(within(form).getByText("Write a question first.")).toBeTruthy();
    await user.type(
      within(form).getByLabelText("Question"),
      "Why is id nullable?",
    );
    const release = hold();
    await user.click(
      within(form).getByRole("button", { name: "Send question" }),
    );
    expect(
      within(form)
        .getByRole("button", { name: "Sending question…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(
      screen.queryByRole("link", { name: /^Question m-40 to/ }),
    ).toBeNull();
    release();
    const markers = await screen.findAllByRole("link", {
      name: "Question m-40 to backend/db about lines 2–3, waiting for an answer",
    });
    expect(markers).toHaveLength(2);
    expect(markers.map((m) => m.getAttribute("href"))).toEqual([
      "/p/demo/messages?thread=m-40",
      "/p/demo/messages?thread=m-40",
    ]);
    expect(markers.map((m) => m.tabIndex)).toEqual([0, -1]);
    expect(screen.queryByRole("form", { name: /^Ask the author/ })).toBeNull();
    expect(
      writes("POST /api/v1/projects/demo/proposals/P-4/questions")[0]?.body,
    ).toEqual({
      version: 2,
      path: SCHEMA,
      side: "new",
      start_line: 2,
      end_line: 3,
      body: "Why is id nullable?",
    });
    expect(writes("POST /api/v1/projects/demo/proposals/P-4/comments")).toEqual(
      [],
    );
    expect(writes("POST /api/v1/projects/demo/proposals/P-4/reviews")).toEqual(
      [],
    );
  });

  it("makes a request from selected lines apart from the review, pending until the daemon answers, then names it and marks the lines", async () => {
    const made: RequestSummary[] = [];
    daemon.on("GET /api/v1/projects/:slug/requests", () => ok({ items: made }));
    daemon.on(
      "POST /api/v1/projects/:slug/requests/from-code",
      async (call) => {
        if (held) await held;
        const body = call.body as {
          path: string;
          start_line: number;
          end_line: number;
          proposal: string;
          version: number;
          side: "old" | "new";
        };
        const code: RequestCode = {
          path: body.path,
          start_line: body.start_line,
          end_line: body.end_line,
          excerpt: "x",
          excerpt_cut: false,
          revision: null,
          proposal: body.proposal,
          version: body.version,
          side: body.side,
        };
        const summary = requestSummary("RQ-7", { code });
        made.push(summary);
        return ok(requestDetail(summary), 201);
      },
    );
    const user = userEvent.setup();
    const { container } = renderAt("/p/demo/proposals/P-4?tab=changes");
    await user.click(await lineCell(container, "new:2"));
    fireEvent.click(await lineCell(container, "new:3"), { shiftKey: true });
    await choose(user, "Make a request");
    const form = await screen.findByRole("form", {
      name: "Make a request about lines 2–3 · new side · v2",
    });
    await user.click(
      within(form).getByRole("button", { name: "Make the request" }),
    );
    expect(within(form).getByText("Say what you need first.")).toBeTruthy();
    await user.type(within(form).getByLabelText("Request"), "Index the date.");
    const release = hold();
    await user.click(
      within(form).getByRole("button", { name: "Make the request" }),
    );
    expect(
      within(form)
        .getByRole("button", { name: "Making the request…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    release();
    const note = await screen.findByText(/with these lines/);
    expect(
      within(note).getByRole("link", { name: "RQ-7" }).getAttribute("href"),
    ).toBe("/p/demo/requests/RQ-7");
    const markers = await screen.findAllByRole("link", {
      name: "Request RQ-7 about lines 2–3, open",
    });
    expect(markers.map((m) => m.getAttribute("href"))).toEqual([
      "/p/demo/requests/RQ-7",
      "/p/demo/requests/RQ-7",
    ]);
    expect(
      writes("POST /api/v1/projects/demo/requests/from-code")[0]?.body,
    ).toEqual({
      body: "Index the date.",
      path: SCHEMA,
      start_line: 2,
      end_line: 3,
      proposal: "P-4",
      version: 2,
      side: "new",
    });
    expect(writes("POST /api/v1/projects/demo/proposals/P-4/comments")).toEqual(
      [],
    );
    expect(
      writes("POST /api/v1/projects/demo/proposals/P-4/questions"),
    ).toEqual([]);
  });

  it("marks the lines of a question on the version it asks about, answered once the author wrote back, and a click on the mark opens the thread without selecting the line", async () => {
    const asked = message("m-24", {
      sender: "human:tanim",
      proposal: "P-4",
      recipients: [recipient("backend/db")],
      lines: {
        version: 1,
        path: SCHEMA,
        side: "new",
        start_line: 4,
        end_line: 4,
        excerpt: "  report_date TEXT",
        excerpt_cut: false,
      },
    });
    questions.set("P-4", [
      thread("m-24", {
        proposal: "P-4",
        messages: [
          asked,
          message("m-28", { thread: "m-24", sender: "backend/db" }),
        ],
      }),
    ]);
    const user = userEvent.setup();
    const current = renderAt("/p/demo/proposals/P-4?tab=changes");
    await lineCell(current.container, "new:4");
    expect(
      screen.queryByRole("link", { name: /^Question m-24 to/ }),
    ).toBeNull();
    cleanup();
    const { container, router } = renderAt(
      "/p/demo/proposals/P-4?tab=changes&version=1",
    );
    const marker = await screen.findByRole("link", {
      name: "Question m-24 to backend/db about line 4, answered",
    });
    await user.click(marker);
    expect(container.querySelector('[aria-selected="true"]')).toBeNull();
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/p/demo/messages");
    });
    expect(router.state.location.search).toEqual({ thread: "m-24" });
  });

  it("offers a question where a version takes no comment, and shows a refusal on the question box", async () => {
    daemon.on(
      "POST /api/v1/projects/:slug/proposals/:id/questions",
      refuse(422, "invalid_comment_range"),
    );
    const user = userEvent.setup();
    const { container } = renderAt(
      "/p/demo/proposals/P-4?tab=changes&version=1",
    );
    await user.click(await lineCell(container, "new:3"));
    const menu = await screen.findByRole("group", { name: "Selected lines" });
    expect(
      within(menu).getByRole("button", { name: "Review comment" }),
    ).toHaveProperty("disabled", true);
    await choose(user, "Ask the author");
    const form = await screen.findByRole("form", {
      name: "Ask the author about line 3 · new side · v1",
    });
    await user.type(within(form).getByLabelText("Question"), "Why?");
    await user.click(
      within(form).getByRole("button", { name: "Send question" }),
    );
    expect(
      await within(form).findByText(
        "The question was not sent. These lines are not in the file on that side.",
      ),
    ).toBeTruthy();
    await user.click(within(form).getByRole("button", { name: "Cancel" }));
    expect(
      await screen.findByRole("group", { name: "Selected lines" }),
    ).toBeTruthy();
  });

  it("disables Approve and Integrate on a marked task until a reviewer agent approved the version", async () => {
    update("P-4", (p) => ({ ...p, agent_approved: false }));
    renderAt("/p/demo/proposals/P-4");
    const bar = await decisions();
    const approve = within(bar).getByRole("button", { name: "Approve v2" });
    expect(approve).toHaveProperty("disabled", true);
    expect(
      within(bar).getAllByText("A reviewer agent approves first."),
    ).toHaveLength(1);
    expect(
      document.getElementById(approve.getAttribute("aria-describedby") ?? "")
        ?.textContent,
    ).toBe("A reviewer agent approves first.");
    expect(
      within(bar).getByRole("button", { name: "Integrate" }),
    ).toHaveProperty("disabled", true);
    expect(writes("POST /api/v1/projects/demo/proposals/P-4/reviews")).toEqual(
      [],
    );
  });

  it("disables Approve while a check of the current version runs, and enables it when the stream reports the end", async () => {
    update("P-4", (p) => ({
      ...p,
      checks: [checkRun(16, { state: "running", finished_at: null })],
    }));
    const { queryClient } = renderAt("/p/demo/proposals/P-4");
    const bar = await decisions();
    const approve = within(bar).getByRole("button", { name: "Approve v2" });
    expect(approve).toHaveProperty("disabled", true);
    expect(
      document.getElementById(approve.getAttribute("aria-describedby") ?? "")
        ?.textContent,
    ).toBe("A check runs on v2. Approve when it ends.");
    expect(
      within(bar).getByRole("button", { name: "Request changes" }),
    ).toHaveProperty("disabled", false);
    update("P-4", (p) => ({ ...p, checks: [checkRun(16)] }));
    deliver(queryClient, "checks.finished", {
      proposal: "P-4",
      task: "T-3",
      data: { run: "cr-16", stage: "proposal", state: "passed" },
    });
    await waitFor(() => {
      expect(
        within(bar).getByRole("button", { name: "Approve v2" }),
      ).toHaveProperty("disabled", false);
    });
    expect(
      within(bar).queryByText("A check runs on v2. Approve when it ends."),
    ).toBeNull();
  });

  it("disables Approve and Integrate while an update runs the checks of the next version, and enables them when the stream reports the end", async () => {
    update("P-4", (p) => ({
      ...p,
      update_run: { run: "cr-18", purpose: "update_proposal" },
    }));
    const { queryClient } = renderAt("/p/demo/proposals/P-4");
    const bar = await decisions();
    const approve = within(bar).getByRole("button", { name: "Approve v2" });
    const integrate = within(bar).getByRole("button", { name: "Integrate" });
    expect(approve).toHaveProperty("disabled", true);
    expect(integrate).toHaveProperty("disabled", true);
    expect(within(bar).getAllByText("v3 is running its checks.")).toHaveLength(
      1,
    );
    expect(
      document.getElementById(approve.getAttribute("aria-describedby") ?? "")
        ?.textContent,
    ).toBe("v3 is running its checks.");
    expect(
      within(bar).getByRole("button", { name: "Request changes" }),
    ).toHaveProperty("disabled", false);
    update("P-4", (p) => ({ ...p, update_run: null }));
    deliver(queryClient, "checks.finished", {
      proposal: "P-4",
      task: "T-3",
      data: {
        run: "cr-18",
        stage: "proposal",
        purpose: "update_proposal",
        state: "failed",
      },
    });
    await waitFor(() => {
      expect(
        within(bar).getByRole("button", { name: "Approve v2" }),
      ).toHaveProperty("disabled", false);
    });
    expect(within(bar).queryByText("v3 is running its checks.")).toBeNull();
  });

  it("disables Integrate while an integration check of the current version runs that no queued request of it started", async () => {
    update("P-4", (p) => ({
      ...p,
      checks: [
        checkRun(17, {
          stage: "integration",
          purpose: "integrate",
          version: 2,
          state: "running",
          finished_at: null,
        }),
      ],
    }));
    renderAt("/p/demo/proposals/P-4");
    const bar = await decisions();
    const integrate = within(bar).getByRole("button", { name: "Integrate" });
    expect(integrate).toHaveProperty("disabled", true);
    expect(
      document.getElementById(integrate.getAttribute("aria-describedby") ?? "")
        ?.textContent,
    ).toBe("fold runs the integration checks of v2. Integrate when they end.");
    expect(
      within(bar).getByRole("button", { name: "Approve v2" }),
    ).toHaveProperty("disabled", true);
    expect(
      within(bar).getByRole("button", { name: "Request changes" }),
    ).toHaveProperty("disabled", false);
  });

  it("shows Approve as pending, then the human approval once the server answers", async () => {
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4");
    const panel = await integration();
    const bar = await decisions();
    expect(
      within(panel).getByText(byContent(/Waiting for @tanim/)),
    ).toBeTruthy();
    const reviews = screen.getByRole("article", { name: "Version v2" });
    const verdicts = () => reviews.querySelectorAll("[data-verdict]").length;
    const before = verdicts();
    const release = hold();
    await user.click(within(bar).getByRole("button", { name: "Approve v2" }));
    expect(
      within(bar)
        .getByRole("button", { name: "Approving v2…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(verdicts()).toBe(before);
    release();
    await waitFor(() => {
      expect(verdicts()).toBe(before + 1);
    });
    expect(
      within(reviews).getByText("@tanim").closest("p")?.textContent,
    ).toMatch(/^Approved@tanim/);
    expect(
      within(panel).getByText(byContent(/Approved by @tanim/)),
    ).toBeTruthy();
    const approved = within(bar).getByRole("button", { name: "Approve v2" });
    expect(approved).toHaveProperty("disabled", true);
    expect(
      document.getElementById(approved.getAttribute("aria-describedby") ?? "")
        ?.textContent,
    ).toBe("You approved v2.");
    expect(
      within(bar).getByRole("button", { name: "Integrate" }),
    ).toHaveProperty("disabled", false);
    expect(
      writes("POST /api/v1/projects/demo/proposals/P-4/reviews")[0]?.body,
    ).toEqual({
      version: 2,
      verdict: "approve",
      body: "approved in the web interface",
    });
  });

  it("needs a body for Request changes, shows it pending, then the verdict once the server answers", async () => {
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4");
    const bar = await decisions();
    await user.click(
      within(bar).getByRole("button", { name: "Request changes" }),
    );
    const form = screen.getByRole("form", { name: "Request changes" });
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
        .getByRole("button", { name: "Sending request…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    const reviews = screen.getByRole("article", { name: "Version v2" });
    release();
    expect(await within(reviews).findByText("Rename the index")).toBeTruthy();
    expect(
      reviews.querySelectorAll('[data-verdict="request_changes"]'),
    ).toHaveLength(1);
    expect(
      screen.getByText("Changes requested · back with the author"),
    ).toBeTruthy();
  });

  it("closes the Request-changes form when the router moves to another proposal", async () => {
    const user = userEvent.setup();
    const { router } = renderAt("/p/demo/proposals/P-4");
    const bar = await decisions();
    await user.click(
      within(bar).getByRole("button", { name: "Request changes" }),
    );
    expect(screen.getByRole("form", { name: "Request changes" })).toBeTruthy();
    await act(async () => {
      await router.navigate({
        to: "/p/$slug/proposals/$id",
        params: { slug: "demo", id: "P-3" },
      });
    });
    await screen.findByText("Login form styles");
    expect(screen.queryByRole("form", { name: "Request changes" })).toBeNull();
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
    const bar = await decisions();
    expect(
      screen.getByText("Changes requested · back with the author"),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: /reject/i })).toBeNull();
    const release = hold();
    await user.click(within(bar).getByRole("button", { name: "Integrate" }));
    expect(
      within(bar)
        .getByRole("button", { name: "Integrating…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(within(panel).queryByText(/Integrated as/)).toBeNull();
    release();
    const link = await within(panel).findByRole("link", { name: "r9" });
    expect(link.getAttribute("href")).toBe("/p/demo/repository?revision=r9");
    expect(screen.getByText("Integrated as r9")).toBeTruthy();
    expect(within(bar).queryByRole("button", { name: "Integrate" })).toBeNull();
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
    const bar = await decisions();
    await user.click(within(bar).getByRole("button", { name: "Integrate" }));
    expect(
      await within(bar).findByText(
        "v3 is now the current version. Read it, then try once more.",
      ),
    ).toBeTruthy();
    expect(
      await screen.findByRole("article", { name: "Version v3" }),
    ).toBeTruthy();
    expect(
      within(bar).getByRole("button", { name: "Approve v3" }),
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
    const bar = await decisions();
    await user.click(within(bar).getByRole("button", { name: "Approve v2" }));
    const banner = await screen.findByText("refused: disk_full");
    expect(banner.closest("[role=alert]")?.textContent).toMatch(
      /The request failed/,
    );
    expect(within(bar).queryByRole("alert")).toBeNull();
  });

  it("shows the result and the new revision after an agent integrates", async () => {
    const { queryClient } = renderAt("/p/demo/proposals/P-4");
    const panel = await integration();
    const bar = await decisions();
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
    expect(
      within(panel).getByText(byContent(/^v2 integrated as r9/)),
    ).toBeTruthy();
    expect(
      within(bar).queryByRole("button", { name: "Approve v2" }),
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
    expect(
      within(report).getByText(byContent(/^P-4 v2 · after line 3$/)),
    ).toBeTruthy();
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
    const bar = await decisions();
    expect(within(panel).getAllByRole("img", { name: "met" })).toHaveLength(3);
    expect(within(panel).getByRole("img", { name: "not met" })).toBeTruthy();
    expect(
      within(panel).getByRole("img", { name: "checked at integration" }),
    ).toBeTruthy();
    expect(within(panel).getByText("Prerequisites not Done: T-1")).toBeTruthy();
    const integrate = within(bar).getByRole("button", { name: "Integrate" });
    expect(integrate.hasAttribute("disabled")).toBe(true);
    expect(integrate.getAttribute("aria-describedby")).toBeTruthy();
    expect(
      within(bar).getByText(
        "Prerequisites not Done: T-1, so you cannot integrate yet.",
      ),
    ).toBeTruthy();
    expect(
      within(bar)
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
          agent_approved: true,
        },
      ),
    );
    renderAt("/p/demo/proposals/P-8");
    const bar = await decisions();
    expect(
      within(bar)
        .getByRole("button", { name: "Integrate" })
        .hasAttribute("disabled"),
    ).toBe(true);
    expect(
      within(bar).getByText("Integrate waits for @ana to approve v1."),
    ).toBeTruthy();
  });

  it("states the base against main, the review claim and the human review mark in the About card", async () => {
    renderAt("/p/demo/proposals/P-4");
    const about = await screen.findByRole("region", { name: "About" });
    const fact = (label: string) =>
      within(about).getByText(label, { selector: "dt" }).nextElementSibling
        ?.textContent;
    await waitFor(() => {
      expect(fact("Base")).toBe("r8 → main r8");
    });
    expect(fact("Review claim")).toBe("@review/quality");
    expect(fact("Human review")).toContain("marked by @backend");
  });

  it("says an open proposal is not claimed and leaves out a human review it has no mark for", async () => {
    put(
      proposalDetail(
        proposalSummary("P-12", {
          title: "Unclaimed",
          review_claim: null,
          human_review: null,
        }),
      ),
    );
    renderAt("/p/demo/proposals/P-12");
    const about = await screen.findByRole("region", { name: "About" });
    expect(
      within(about).getByText("Review claim", { selector: "dt" })
        .nextElementSibling?.textContent,
    ).toBe("not claimed");
    expect(
      within(about).queryByText("Human review", { selector: "dt" }),
    ).toBeNull();
  });

  it("says when the project has no such proposal", async () => {
    renderAt("/p/demo/proposals/P-99");
    expect(
      await screen.findByRole("region", { name: "Proposal P-99 not found" }),
    ).toBeTruthy();
  });
});

describe("Proposal tabs", () => {
  it("opens the overview by default, keeps the tab in the address, and opens the change for an address with a file", async () => {
    const user = userEvent.setup();
    const { router } = renderAt("/p/demo/proposals/P-4");
    expect(
      (await screen.findByRole("tab", { name: "Overview" })).getAttribute(
        "aria-selected",
      ),
    ).toBe("true");
    await user.click(screen.getByRole("tab", { name: /^Changes/ }));
    await waitFor(() => {
      expect(router.state.location.search).toEqual({ tab: "changes" });
    });
    expect(
      await screen.findByRole("navigation", { name: "Changed files" }),
    ).toBeTruthy();
    await user.click(screen.getByRole("tab", { name: /^Conversation/ }));
    await waitFor(() => {
      expect(router.state.location.search).toEqual({ tab: "conversation" });
    });
    cleanup();
    renderAt(`/p/demo/proposals/P-4?path=${encodeURIComponent(SCHEMA)}`);
    expect(
      (await screen.findByRole("tab", { name: /^Changes/ })).getAttribute(
        "aria-selected",
      ),
    ).toBe("true");
  });

  it("adds a history entry for each tab, so Back returns to the earlier tab, and replaces it for a version on the same tab", async () => {
    const user = userEvent.setup();
    const { router } = renderAt("/p/demo/proposals/P-4");
    await screen.findByRole("tab", { name: "Overview" });
    await user.click(screen.getByRole("tab", { name: /^Checks/ }));
    await waitFor(() => {
      expect(router.state.location.search).toEqual({ tab: "checks" });
    });
    await user.click(screen.getByRole("tab", { name: /^Changes/ }));
    await waitFor(() => {
      expect(router.state.location.search).toEqual({ tab: "changes" });
    });
    router.history.back();
    await waitFor(() => {
      expect(router.state.location.search).toEqual({ tab: "checks" });
    });
    expect(
      screen
        .getByRole("tab", { name: /^Checks/ })
        .getAttribute("aria-selected"),
    ).toBe("true");
    router.history.back();
    await waitFor(() => {
      expect(router.state.location.search).toEqual({});
    });
    await user.click(screen.getByRole("tab", { name: /^Changes/ }));
    await user.click(
      await screen.findByRole("button", { name: /^Version: v2/ }),
    );
    await user.click(await screen.findByRole("menuitem", { name: /^v1/ }));
    await waitFor(() => {
      expect(router.state.location.search).toEqual({
        tab: "changes",
        version: 1,
      });
    });
    router.history.back();
    await waitFor(() => {
      expect(router.state.location.search).toEqual({});
    });
  });

  it("shows a review with the comments it came with in its dialog", async () => {
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4");
    const v1 = await screen.findByRole("article", { name: "Version v1" });
    await user.click(
      await within(v1).findByRole("button", { name: "Read the full review" }),
    );
    const dialog = await screen.findByRole("dialog");
    const listed = within(dialog).getByRole("region", {
      name: "Comments of the review",
    });
    expect(
      within(listed).getByText("Add an index on (owner_id, report_date)."),
    ).toBeTruthy();
    expect(
      within(listed).getByText(byContent(`rc-1 · ${SCHEMA} · line 3`)),
    ).toBeTruthy();
    expect(
      within(listed).queryByText("Added reports_owner_date_idx in v2."),
    ).toBeNull();
  });

  it("says when the checks of an earlier version could not be read", async () => {
    daemon.on("GET /api/v1/projects/:slug/proposals/:id", (call) => {
      const found = proposals.get(call.params.id ?? "");
      if (!found) return refuse(404, "proposal_not_found");
      return call.path.includes("version=1")
        ? refuse(500, "disk_full")
        : ok(found);
    });
    renderAt("/p/demo/proposals/P-4");
    const v1 = await screen.findByRole("article", { name: "Version v1" });
    expect(
      await within(v1).findByText("The checks could not be read."),
    ).toBeTruthy();
    const v2 = screen.getByRole("article", { name: "Version v2" });
    expect(within(v2).queryByText("The checks could not be read.")).toBeNull();
  });

  it("lists the changed files as folders that close, open, filter and mark the files the reader opens as viewed", async () => {
    window.localStorage.clear();
    const user = userEvent.setup();
    const { container } = renderAt("/p/demo/proposals/P-4?tab=changes");
    await lineCell(container, "new:1");
    const tree = screen.getByRole("navigation", { name: "Changed files" });
    const folder = within(tree).getByRole("button", { name: /migrations/ });
    expect(folder.getAttribute("aria-expanded")).toBe("true");
    expect(
      within(tree).getByRole("button", { name: /0004_reports\.sql/ }),
    ).toBeTruthy();
    await user.click(folder);
    expect(folder.getAttribute("aria-expanded")).toBe("false");
    expect(
      within(tree).queryByRole("button", { name: /0004_reports\.sql/ }),
    ).toBeNull();
    await user.click(within(tree).getByRole("button", { name: "Expand all" }));
    expect(
      within(tree).getByRole("button", { name: /0004_reports\.sql/ }),
    ).toBeTruthy();
    await user.click(
      within(tree).getByRole("button", { name: "Collapse all" }),
    );
    expect(
      within(tree).queryByRole("button", { name: /0004_reports\.sql/ }),
    ).toBeNull();
    await user.type(within(tree).getByLabelText("Files"), "reports");
    expect(
      within(tree).getByRole("button", { name: /0004_reports\.sql/ }),
    ).toBeTruthy();
    await user.clear(within(tree).getByLabelText("Files"));
    await user.type(within(tree).getByLabelText("Files"), "nothing");
    expect(within(tree).getByText("No file matches.")).toBeTruthy();
    await user.clear(within(tree).getByLabelText("Files"));
    await user.click(within(tree).getByRole("button", { name: "Expand all" }));
    const row = within(tree).getByRole("button", {
      name: /0004_reports\.sql/,
    });
    expect(within(tree).getByText("1 of 1 viewed")).toBeTruthy();
    expect(row.getAttribute("data-viewed")).toBe("true");
    expect(window.localStorage.getItem("fold.viewed.demo.P-4.base..v2")).toBe(
      JSON.stringify([SCHEMA]),
    );
    await user.click(screen.getByRole("checkbox", { name: "Viewed" }));
    expect(within(tree).getByText("0 of 1 viewed")).toBeTruthy();
    expect(row.getAttribute("data-viewed")).toBeNull();
    expect(window.localStorage.getItem("fold.viewed.demo.P-4.base..v2")).toBe(
      "[]",
    );
    await user.click(row);
    expect(within(tree).getByText("1 of 1 viewed")).toBeTruthy();
  });

  it("says when the conversation left out older message threads", async () => {
    daemon.on("GET /api/v1/projects/:slug/messages", () =>
      ok({ items: [], next: "older", total_awaiting_human: 0 }),
    );
    renderAt("/p/demo/proposals/P-4?tab=conversation");
    expect(
      await screen.findByText(
        "Showing the newest 0 message threads. Older message threads are not listed.",
      ),
    ).toBeTruthy();
  });

  it("asks the author about the whole proposal from the decision bar, pending until the daemon answers, then closes the panel", async () => {
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4?tab=conversation");
    const bar = await decisions();
    expect(screen.queryByRole("form", { name: "Ask the author" })).toBeNull();
    await user.click(
      within(bar).getByRole("button", { name: "Ask the author" }),
    );
    const form = await screen.findByRole("form", { name: "Ask the author" });
    expect(screen.queryByRole("checkbox")).toBeNull();
    await user.type(
      within(form).getByLabelText("Question to the author"),
      "Why one table?",
    );
    const release = hold();
    await user.click(
      within(form).getByRole("button", { name: "Ask the author" }),
    );
    expect(
      within(form)
        .getByRole("button", { name: "Asking…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    release();
    const asked = await screen.findByRole("article", {
      name: "Question to the author m-40",
    });
    expect(within(asked).getByText("whole proposal")).toBeTruthy();
    expect(within(asked).getByText("Why one table?")).toBeTruthy();
    expect(screen.queryByRole("form", { name: "Ask the author" })).toBeNull();
    expect(
      writes("POST /api/v1/projects/demo/proposals/P-4/questions")[0]?.body,
    ).toEqual({ version: 2, body: "Why one table?" });
  });

  it("filters the conversation by state, kind and version, and shows a forward to the area lead only", async () => {
    questions.set("P-4", [
      thread("m-24", {
        proposal: "P-4",
        awaiting_human: true,
        last_at: 5,
        messages: [
          message("m-24", {
            sender: "backend/db",
            proposal: "P-4",
            asks_human: true,
            recipients: [recipient("human:tanim", { addressed_as: "human" })],
            body: "Keep the old column?",
            sent_at: Date.now(),
          }),
        ],
      }),
      thread("m-32", {
        proposal: "P-4",
        last_at: 4,
        messages: [
          message("m-32", {
            sender: "human:tanim",
            proposal: "P-4",
            recipients: [
              recipient("backend/db", { forwarded_to: "db-lead" }),
              recipient("db-lead"),
            ],
            body: "Why TEXT dates?",
            sent_at: Date.now(),
          }),
          message("m-28", {
            thread: "m-32",
            sender: "fold",
            proposal: "P-4",
            recipients: [recipient("human:tanim"), recipient("db-lead")],
            body: "@backend/db cannot start now, so fold sent m-32 on to @db-lead.",
            sent_at: Date.now(),
          }),
        ],
      }),
    ]);
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4?tab=conversation");
    expect(
      await screen.findByRole("article", { name: "Question to you m-24" }),
    ).toBeTruthy();
    const waiting = screen.getByRole("article", {
      name: "Question to the author m-32",
    });
    expect(
      within(waiting).getByText(byContent(/so fold sent m-32 on to @db-lead/)),
    ).toBeTruthy();
    expect(within(waiting).queryByText(/delivered|waiting for/)).toBeNull();
    expect(screen.getAllByRole("article")).toHaveLength(3);
    const filters = screen.getByRole("complementary", { name: "Filters" });
    await user.click(
      within(filters).getByRole("button", { name: /^Needs your answer/ }),
    );
    expect(
      screen.getAllByRole("article").map((a) => a.getAttribute("aria-label")),
    ).toEqual(["Question to you m-24"]);
    expect(
      within(filters)
        .getAllByRole("button", { pressed: false })
        .map((chip) => chip.textContent),
    ).not.toContain("Open0");
    await user.click(within(filters).getByRole("button", { name: /^All ?\d/ }));
    await user.click(
      within(filters).getByRole("button", { name: "Kind: Any kind" }),
    );
    expect(
      (
        await screen.findByRole("menuitemradio", { name: /^Any kind/ })
      ).getAttribute("aria-checked"),
    ).toBe("true");
    await user.click(
      screen.getByRole("menuitemradio", { name: /^Review comments/ }),
    );
    expect(
      screen.getAllByRole("article").map((a) => a.getAttribute("aria-label")),
    ).toEqual(["Review comment rc-1"]);
    await user.click(
      within(filters).getByRole("button", { name: "Kind: Review comments" }),
    );
    expect(
      (
        await screen.findByRole("menuitemradio", { name: /^Review comments/ })
      ).getAttribute("aria-checked"),
    ).toBe("true");
    await user.click(screen.getByRole("menuitemradio", { name: /^Any kind/ }));
    expect(screen.getAllByRole("article")).toHaveLength(3);
    await user.click(
      within(filters).getByRole("button", { name: "Version: All versions" }),
    );
    await user.click(
      await screen.findByRole("menuitemradio", { name: /^v1 ?\d/ }),
    );
    expect(
      screen.getAllByRole("article").map((a) => a.getAttribute("aria-label")),
    ).toEqual(["Review comment rc-1"]);
  });
});

describe("Human edit", () => {
  const EDITS = "POST /api/v1/projects/demo/proposals/P-4/edits";
  const AGAIN = "POST /api/v1/projects/demo/proposals/P-4/review-again";

  function editRoutes(): void {
    daemon.on("GET /api/v1/projects/:slug/proposals/:id/file", () =>
      ok({
        path: SCHEMA,
        kind: "file",
        hash: "f".repeat(64),
        size: 26,
        binary: false,
        too_large: false,
        text: "CREATE TABLE reports (\r\n);\r\n",
      }),
    );
    daemon.on("POST /api/v1/projects/:slug/proposals/:id/edits", async () => {
      if (held) await held;
      const made = proposalVersion(3, {
        created_by: "human:tanim",
        human_edit: true,
      });
      update("P-4", (p) => ({
        ...p,
        summary: { ...p.summary, current_version: 3 },
        versions: [...p.versions, made],
        reviews: [
          ...p.reviews,
          review(3, "review/quality", "approve", {
            kept_from: 2,
            created_at: Date.now(),
          }),
        ],
      }));
      return ok(made, 201);
    });
    daemon.on(
      "POST /api/v1/projects/:slug/proposals/:id/review-again",
      async () => {
        if (held) await held;
        update("P-4", (p) => ({
          ...p,
          summary: { ...p.summary, latest_verdict: null },
          versions: p.versions.map((v) =>
            v.version === 3 ? { ...v, review_again: true } : v,
          ),
          reviews: p.reviews.filter((r) => r.kept_from === null),
          agent_approved: false,
        }));
        return ok(proposals.get("P-4")?.summary);
      },
    );
  }

  it("edits a file of the current version in a human review: pending, then the new version with its kept approval", async () => {
    editRoutes();
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4?tab=changes");
    await user.click(
      await screen.findByRole("button", { name: `Edit ${SCHEMA}` }),
    );
    const editor = await screen.findByRole("region", {
      name: `Edit ${SCHEMA}`,
    });
    expect(
      within(editor).getByText(
        "You are editing v2. Saving writes v3 under your name; v2 stays in the history. v3 keeps the agent approvals of v2, and you approve v3 again.",
      ),
    ).toBeTruthy();
    const save = within(editor).getByRole("button", { name: "Save as v3" });
    expect(save).toHaveProperty("disabled", true);
    const box = within(editor).getByLabelText(`Content of ${SCHEMA}`);
    expect((box as HTMLTextAreaElement).value).toBe(
      "CREATE TABLE reports (\n);\n",
    );
    fireEvent.change(box, {
      target: { value: "CREATE TABLE reports (\n);\n-- checked\n" },
    });
    expect(save).toHaveProperty("disabled", true);
    const form = within(editor).getByRole("region", { name: "Intent of v3" });
    expect(
      within(form).getByText(
        "How v3 works in the code, and its parts: every file v3 changes belongs to exactly one part. v2 has no intent, so write one for v3.",
      ),
    ).toBeTruthy();
    expect(
      within(form).getByLabelText<HTMLTextAreaElement>("Files of part 1").value,
    ).toBe(SCHEMA);
    writeIntent(form, "Marks the reports table as checked.");
    expect(save).toHaveProperty("disabled", false);
    const release = hold();
    await user.click(save);
    expect(
      within(editor)
        .getByRole("button", { name: "Saving v3…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(screen.queryByText("Approved (kept from v2)")).toBeNull();
    release();
    await waitFor(() => {
      expect(
        screen.queryByRole("region", { name: `Edit ${SCHEMA}` }),
      ).toBeNull();
    });
    const panel = await screen.findByRole("group", { name: "Your review" });
    expect(
      await within(panel).findByRole("button", { name: "Approve v3" }),
    ).toBeTruthy();
    expect(
      within(panel).getByText(
        "v3 is a human edit. It keeps the agent approvals of v2 until it is sent for review again.",
      ),
    ).toBeTruthy();
    await user.click(screen.getByRole("tab", { name: "Overview" }));
    const v3 = await screen.findByRole("article", { name: "Version v3" });
    expect(within(v3).getByText("Approved (kept from v2)")).toBeTruthy();
    expect(writes(EDITS)[0]?.body).toEqual({
      version: 2,
      path: SCHEMA,
      content: "CREATE TABLE reports (\r\n);\r\n-- checked\r\n",
      intent: intent("Marks the reports table as checked.", [
        ["Reports table", "Adds a check note to the table.", [SCHEMA]],
      ]),
    });
  });

  it("sends a human edit for review again: pending, then the kept approval no longer shows", async () => {
    editRoutes();
    update("P-4", (p) => ({
      ...p,
      summary: { ...p.summary, current_version: 3 },
      versions: [
        ...p.versions,
        proposalVersion(3, { created_by: "human:tanim", human_edit: true }),
      ],
      reviews: [
        ...p.reviews,
        review(3, "review/quality", "approve", { kept_from: 2 }),
      ],
    }));
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4");
    const panel = await decisions();
    const release = hold();
    await user.click(
      within(panel).getByRole("button", { name: "Send v3 for review again" }),
    );
    expect(
      within(panel)
        .getByRole("button", { name: "Sending for review…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    release();
    await waitFor(() => {
      expect(screen.queryByText("Approved (kept from v2)")).toBeNull();
    });
    expect(
      within(panel).queryByRole("button", { name: "Send v3 for review again" }),
    ).toBeNull();
    expect(writes(AGAIN)[0]?.body).toEqual({ version: 3 });
  });

  it("shows a refusal of the edit on the edit box", async () => {
    editRoutes();
    daemon.on("POST /api/v1/projects/:slug/proposals/:id/edits", () =>
      refuse(409, "worker_busy", { reason: "working" }),
    );
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4?tab=changes");
    await user.click(
      await screen.findByRole("button", { name: `Edit ${SCHEMA}` }),
    );
    const editor = await screen.findByRole("region", {
      name: `Edit ${SCHEMA}`,
    });
    fireEvent.change(within(editor).getByLabelText(`Content of ${SCHEMA}`), {
      target: { value: "x\n" },
    });
    writeIntent(
      within(editor).getByRole("region", { name: "Intent of v3" }),
      "Clears the table.",
    );
    await user.click(
      within(editor).getByRole("button", { name: "Save as v3" }),
    );
    expect(
      await within(editor).findByText(
        "The task worker is working in its workspace now. Try again when it stops.",
      ),
    ).toBeTruthy();
    await user.click(within(editor).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("region", { name: `Edit ${SCHEMA}` })).toBeNull();
  });

  it("starts the intent of the edit from the edited version and sends the one the human writes", async () => {
    editRoutes();
    const kept = intent("Adds the reports table.", [
      ["Reports table", "Creates the table.", [SCHEMA]],
    ]);
    update("P-4", (p) => ({
      ...p,
      versions: p.versions.map((v) =>
        v.version === 2 ? { ...v, intent: kept } : v,
      ),
    }));
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4?tab=changes");
    await user.click(
      await screen.findByRole("button", { name: `Edit ${SCHEMA}` }),
    );
    const editor = await screen.findByRole("region", {
      name: `Edit ${SCHEMA}`,
    });
    fireEvent.change(within(editor).getByLabelText(`Content of ${SCHEMA}`), {
      target: { value: "CREATE TABLE reports (\n);\n-- checked\n" },
    });
    const form = within(editor).getByRole("region", { name: "Intent of v3" });
    const summary = within(form).getByLabelText("Summary");
    expect((summary as HTMLTextAreaElement).value).toBe(
      "Adds the reports table.",
    );
    expect(
      within(form).getByLabelText<HTMLInputElement>("Part 1 name").value,
    ).toBe("Reports table");
    const save = within(editor).getByRole("button", { name: "Save as v3" });
    expect(save).toHaveProperty("disabled", false);
    fireEvent.change(summary, { target: { value: " " } });
    expect(save).toHaveProperty("disabled", true);
    expect(
      within(form).getByText(
        "Write the summary: how the change works in the code.",
      ),
    ).toBeTruthy();
    fireEvent.change(within(form).getByLabelText("Files of part 1"), {
      target: { value: "" },
    });
    fireEvent.change(summary, {
      target: { value: "Adds the reports table, checked by a human." },
    });
    expect(
      within(form).getByText(
        `Put ${SCHEMA} in the part whose code it changes.`,
      ),
    ).toBeTruthy();
    fireEvent.change(within(form).getByLabelText("Files of part 1"), {
      target: { value: SCHEMA },
    });
    fireEvent.change(within(form).getByLabelText("Decisions"), {
      target: { value: "A human checked the table.\n" },
    });
    await user.click(save);
    await waitFor(() => {
      expect(writes(EDITS)).toHaveLength(1);
    });
    expect(writes(EDITS)[0]?.body).toEqual({
      version: 2,
      path: SCHEMA,
      content: "CREATE TABLE reports (\r\n);\r\n-- checked\r\n",
      intent: {
        ...kept,
        summary: "Adds the reports table, checked by a human.",
        decisions: ["A human checked the table."],
      },
    });
  });

  it("keeps the edited version's intent when the human leaves it, and shows a refusal of its parts", async () => {
    editRoutes();
    const kept = intent("Adds the reports table.", [
      ["Reports table", "Creates the table.", [SCHEMA]],
    ]);
    update("P-4", (p) => ({
      ...p,
      versions: p.versions.map((v) =>
        v.version === 2 ? { ...v, intent: kept } : v,
      ),
    }));
    daemon.on("POST /api/v1/projects/:slug/proposals/:id/edits", () =>
      refuse(400, "invalid_intent", {
        field: "intent.parts",
        reason: "part_file_outside_diff",
        outside: [SCHEMA],
      }),
    );
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4?tab=changes");
    await user.click(
      await screen.findByRole("button", { name: `Edit ${SCHEMA}` }),
    );
    const editor = await screen.findByRole("region", {
      name: `Edit ${SCHEMA}`,
    });
    fireEvent.change(within(editor).getByLabelText(`Content of ${SCHEMA}`), {
      target: { value: "x\n" },
    });
    await user.click(
      within(editor).getByRole("button", { name: "Save as v3" }),
    );
    expect(
      await within(editor).findByText(
        `The version does not change these files: ${SCHEMA}. Remove them from the parts.`,
      ),
    ).toBeTruthy();
    expect(writes(EDITS)[0]?.body).toEqual({
      version: 2,
      path: SCHEMA,
      content: "x\r\n",
    });
  });

  it("offers no edit box outside a human review", async () => {
    const { container } = renderAt("/p/demo/proposals/P-3?tab=changes");
    await lineCell(container, "new:1");
    expect(screen.queryByRole("button", { name: /^Edit / })).toBeNull();
  });
});

describe("Checks", () => {
  const passed = {
    name: "unit",
    setup: false,
    required: true,
    outcome: "passed" as const,
    exit_code: 0,
    duration_ms: 1200,
    log_bytes: 8,
    log: "",
    truncated: false,
  };
  function checkRun(
    id: string,
    fields: Partial<ProposalDetail["checks"][number]> = {},
  ) {
    return {
      id,
      stage: "proposal" as const,
      purpose: "create_proposal" as const,
      task: "T-3",
      version: null,
      base: "r8",
      state: "passed" as const,
      reason: null,
      created_at: 1,
      finished_at: 2,
      batch: null,
      results: [passed],
      ...fields,
    };
  }

  it("lists the check runs of the version and reads a failed check's log end", async () => {
    update("P-4", (p) => ({
      ...p,
      summary: { ...p.summary, checks_failed: true, turn: "with_author" },
      checks: [
        checkRun("cr-9", {
          stage: "integration",
          purpose: "integrate",
          version: 2,
          state: "failed",
          reason: "check_failed",
          results: [
            {
              ...passed,
              name: "e2e",
              outcome: "failed",
              exit_code: 1,
              log_bytes: 20,
            },
          ],
        }),
        checkRun("cr-7"),
      ],
    }));
    daemon.on("GET /api/v1/projects/:slug/check-runs/:id", (call) =>
      ok(
        checkRun(call.params.id ?? "", {
          results: [
            {
              ...passed,
              name: "e2e",
              outcome: "failed",
              exit_code: 1,
              log: "step one\nassertion failed\n",
            },
          ],
        }),
      ),
    );
    const user = userEvent.setup();
    const { router } = renderAt("/p/demo/proposals/P-4?tab=checks");
    const checks = await screen.findByRole("navigation", {
      name: "Check runs",
    });
    expect(
      within(checks).getByText("v2 integration checks on r8"),
    ).toBeTruthy();
    expect(within(checks).getByText("v2 proposal checks")).toBeTruthy();
    expect(within(checks).getByText("a required check failed")).toBeTruthy();
    expect(within(checks).getByText("exit 1 · 1.2 s")).toBeTruthy();
    const log = await screen.findByRole("region", { name: "Log of e2e" });
    const error = await within(log).findByText("assertion failed");
    expect(error.closest("li")?.getAttribute("data-error")).toBe("true");
    expect(
      within(log)
        .getByText("step one")
        .closest("li")
        ?.hasAttribute("data-error"),
    ).toBe(false);
    expect(within(log).getByText(/failed · exit 1 · 1\.2 s/)).toBeTruthy();
    expect(
      daemon.made("GET /api/v1/projects/demo/check-runs/cr-9"),
    ).toHaveLength(1);
    await user.click(within(checks).getByRole("link", { name: /unit/ }));
    await waitFor(() => {
      expect(router.state.location.search).toEqual({
        tab: "checks",
        run: "cr-7",
        step: "unit",
      });
    });
    expect(
      await screen.findByRole("region", { name: "Log of unit" }),
    ).toBeTruthy();
    await user.click(screen.getByRole("tab", { name: "Overview" }));
    const history = await screen.findByRole("article", { name: "Version v2" });
    expect(within(history).getByText(/e2e failed \(exit 1\)/)).toBeTruthy();
    const panel = await integration();
    const bar = await decisions();
    expect(
      within(panel).getByText(
        "The integration checks of v2 failed on the merge with main. The author sends a new version.",
      ),
    ).toBeTruthy();
    expect(
      within(bar).getByRole("button", { name: "Approve v2" }),
    ).toHaveProperty("disabled", true);
    expect(
      within(bar).getByRole("button", { name: "Integrate" }),
    ).toHaveProperty("disabled", true);
    expect(
      within(bar).getAllByText(
        "The integration checks failed; the author sends a new version.",
      ),
    ).toHaveLength(1);
    expect(
      within(bar).getByRole("button", { name: "Request changes" }),
    ).toHaveProperty("disabled", false);
  });

  it("says when fold ran no check on the version", async () => {
    renderAt("/p/demo/proposals/P-3");
    const history = await screen.findByRole("article", { name: "Version v1" });
    expect(
      within(history).getByText("fold ran no check on this version."),
    ).toBeTruthy();
    cleanup();
    renderAt("/p/demo/proposals/P-3?tab=checks");
    expect(
      await screen.findByRole("region", { name: "No check runs" }),
    ).toBeTruthy();
  });

  it("shows a queued integration as waiting for its checks until the stream brings the result", async () => {
    daemon.on("POST /api/v1/projects/:slug/proposals/:id/integrate", (call) => {
      update("P-4", (p) => ({
        ...p,
        summary: { ...p.summary, integration_queued: true },
      }));
      return ok({
        proposal: call.params.id,
        result: "queued",
        revision: null,
        run: null,
      });
    });
    const user = userEvent.setup();
    const { queryClient } = renderAt("/p/demo/proposals/P-4");
    const panel = await integration();
    const bar = await decisions();
    await user.click(within(bar).getByRole("button", { name: "Integrate" }));
    expect(
      await within(panel).findByText(
        "Queued: fold runs the integration checks on the merge with main, then integrates when they pass.",
      ),
    ).toBeTruthy();
    expect(
      within(bar).getByText("This integration is already queued."),
    ).toBeTruthy();
    expect(
      within(bar).getByRole("button", { name: "Integrate" }),
    ).toHaveProperty("disabled", true);
    integrate("P-4", 9);
    deliver(queryClient, "proposal.integrated", {
      proposal: "P-4",
      task: "T-3",
      revision: "r9",
      data: { version: 2 },
    });
    expect(await within(panel).findByRole("link", { name: "r9" })).toBeTruthy();
    expect(within(panel).queryByText(/^Queued:/)).toBeNull();
  });

  it("shows an integration someone else queued after a reload, and lets it go when fold takes it out of the queue", async () => {
    update("P-4", (p) => ({
      ...p,
      summary: { ...p.summary, integration_queued: true },
    }));
    const { queryClient } = renderAt("/p/demo/proposals/P-4");
    const panel = await integration();
    const bar = await decisions();
    expect(await within(panel).findByText(/^Queued:/)).toBeTruthy();
    expect(
      within(bar).getByRole("button", { name: "Integrate" }),
    ).toHaveProperty("disabled", true);
    update("P-4", (p) => ({
      ...p,
      summary: { ...p.summary, integration_queued: false },
    }));
    deliver(queryClient, "integration.unqueued", {
      proposal: "P-4",
      task: "T-3",
      actor: "fold",
      data: { version: 2 },
    });
    await waitFor(() => {
      expect(within(panel).queryByText(/^Queued:/)).toBeNull();
    });
    expect(
      within(bar).getByRole("button", { name: "Integrate" }),
    ).toHaveProperty("disabled", false);
  });
});

describe("Intent", () => {
  const LARGE = intent(
    "A candidate opens one case per kind at POST /v1/cases, linked to #T-9.\n\nStaff decide it.",
    [
      ["Reports table", "Creates the reports table.", [SCHEMA]],
      ["Docs", "Describes the cases.", ["docs/cases.md"]],
    ],
    {
      contracts: {
        routes: ["POST /v1/cases"],
        database: ["New table reports"],
        rights: [],
        config: [],
        errors: ["case_open"],
        events: [],
        commands: [],
      },
      decisions: ["One open case per kind."],
      not_changed: ["Sign-in."],
    },
  );

  it("shows the current version's intent part by part first, the reviewer's verdict beside it and each earlier version's summary in the history", async () => {
    update("P-4", (p) => ({
      ...p,
      versions: p.versions.map((v) => ({
        ...v,
        intent:
          v.version === 2
            ? LARGE
            : intent("Adds the reports table.", [
                ["Table", "Creates it.", [SCHEMA]],
              ]),
      })),
      reviews: p.reviews.map((r) =>
        r.version === 2 ? { ...r, body: "The diff matches the intent." } : r,
      ),
    }));
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4");
    const card = await screen.findByRole("region", { name: "Intent" });
    expect(within(card).getByText("Intent of v2")).toBeTruthy();
    expect(within(card).getByText(/checked by/)).toBeTruthy();
    expect(within(card).getByText(/A candidate opens one case/)).toBeTruthy();
    expect(
      within(card).getByRole("link", { name: "T-9" }).getAttribute("href"),
    ).toBe("/p/demo/t/T-9");
    expect(
      within(card).getByText("Both changed files belong to a part below."),
    ).toBeTruthy();
    const parts = within(card).getByRole("region", { name: "Parts" });
    expect(within(parts).getByText("Reports table")).toBeTruthy();
    expect(within(parts).getByText("Describes the cases.")).toBeTruthy();
    expect(
      within(parts).getByRole("link", { name: SCHEMA }).getAttribute("href"),
    ).toContain("tab=changes");
    const contracts = within(card).getByRole("region", { name: "Contracts" });
    expect(within(contracts).getByText("Routes")).toBeTruthy();
    expect(within(contracts).getByText("case_open")).toBeTruthy();
    expect(within(contracts).queryByText("Rights")).toBeNull();
    expect(
      within(card).getByRole("region", { name: "Decisions" }),
    ).toBeTruthy();
    expect(
      within(
        within(card).getByRole("region", { name: "What it does not change" }),
      ).getByText("Sign-in."),
    ).toBeTruthy();
    const verdict = screen.getByRole("region", { name: "Review" });
    expect(within(verdict).getByText("Approved")).toBeTruthy();
    expect(
      within(verdict).getByText("The diff matches the intent."),
    ).toBeTruthy();
    const integration = screen.getByRole("region", { name: "Integration" });
    expect(
      card.compareDocumentPosition(integration) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    const v1 = screen.getByRole("article", { name: "Version v1" });
    expect(within(v1).getByText("Adds the reports table.")).toBeTruthy();
    expect(within(v1).queryByText("Creates it.")).toBeNull();
    await user.click(
      within(v1).getByRole("button", { name: "Show the whole intent of v1" }),
    );
    expect(within(v1).getByText("Creates it.")).toBeTruthy();
    await user.click(
      within(v1).getByRole("button", { name: "Hide the parts" }),
    );
    expect(within(v1).queryByText("Creates it.")).toBeNull();
    const v2 = screen.getByRole("article", { name: "Version v2" });
    expect(within(v2).queryByText(/A candidate opens/)).toBeNull();
  });

  it("shows a small change with one part and no other section", async () => {
    update("P-4", (p) => ({
      ...p,
      versions: p.versions.map((v) => ({
        ...v,
        intent: intent("Resets the page when a filter changes.", [
          ["Reset the page", "setFilter writes page 1.", [SCHEMA]],
        ]),
      })),
    }));
    renderAt("/p/demo/proposals/P-4");
    const card = await screen.findByRole("region", { name: "Intent" });
    expect(
      within(card).getByText("The changed file belongs to the part below."),
    ).toBeTruthy();
    expect(
      within(card).queryByRole("region", { name: "Contracts" }),
    ).toBeNull();
    expect(
      within(card).queryByRole("region", { name: "Decisions" }),
    ).toBeNull();
    expect(
      within(card).queryByRole("region", { name: "What it does not change" }),
    ).toBeNull();
  });

  it("groups the changed files by part and shows the part above the file", async () => {
    update("P-4", (p) => ({
      ...p,
      versions: p.versions.map((v) => ({ ...v, intent: LARGE })),
    }));
    renderAt(`/p/demo/proposals/P-4?tab=changes&path=${SCHEMA}`);
    const tree = await screen.findByRole("navigation", {
      name: "Changed files",
    });
    expect(within(tree).getByText("Reports table")).toBeTruthy();
    expect(within(tree).queryByText("Not in the intent")).toBeNull();
    expect(
      within(tree).queryByRole("button", { name: "Expand all" }),
    ).toBeNull();
    const file = await screen.findByRole("region", { name: SCHEMA });
    expect(
      within(file).getByText("Creates the reports table.", { exact: false }),
    ).toBeTruthy();
  });

  it("warns about a changed file that no part names", async () => {
    update("P-4", (p) => ({
      ...p,
      versions: p.versions.map((v) => ({
        ...v,
        intent: intent("Describes the cases.", [
          ["Docs", "Describes the cases.", ["docs/cases.md"]],
        ]),
      })),
    }));
    renderAt(`/p/demo/proposals/P-4?tab=changes&path=${SCHEMA}`);
    const tree = await screen.findByRole("navigation", {
      name: "Changed files",
    });
    expect(within(tree).getByText("Not in the intent")).toBeTruthy();
    const file = await screen.findByRole("region", { name: SCHEMA });
    expect(
      within(file).getByText("belongs to no part of the intent.", {
        exact: false,
      }),
    ).toBeTruthy();
  });

  it("says when a version has no intent and nobody reviewed it", async () => {
    update("P-4", (p) => ({ ...p, reviews: [] }));
    renderAt("/p/demo/proposals/P-4");
    const card = await screen.findByRole("region", { name: "Intent" });
    expect(within(card).getByText("This version has no intent.")).toBeTruthy();
    expect(
      within(screen.getByRole("region", { name: "Review" })).getByText(
        "No reviewer has reviewed v2 yet.",
      ),
    ).toBeTruthy();
  });
});
