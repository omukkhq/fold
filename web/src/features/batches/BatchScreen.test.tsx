// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import {
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { keys } from "@/api/keys";
import type { Session } from "@/api/session";
import type { ReviewBatch } from "@/api/reviews";
import { errorReply, stubFetch, type StubReply } from "@/test/stubFetch";
import {
  batch,
  member,
  proposalSummary,
  workerList,
  workerProcess,
} from "@/test/workers";
import { BatchScreen } from "./BatchScreen";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const BASE = "/api/v1/projects/demo";

const rb3 = batch({
  checkout: { base: "r17", path: ".fold/ws/review-rb-3", workspace: "ws-12" },
  members: [
    member("P-3", { position: 1, version: 2, current_version: 2 }),
    member("P-4", { position: 2, task: "T-11" }),
  ],
  verdicts: [{ proposal: "P-3", verdict: "approve", version: 2 }],
});

const reviewer = workerProcess("review-b3", {
  role: "reviewer",
  area: null,
  model: "sonnet",
  turns: 21,
});

async function setup(
  shown: ReviewBatch,
  extra: Record<
    string,
    StubReply | (() => StubReply | Promise<StubReply>)
  > = {},
  session: Session = { human: "tanim", authenticated: true },
) {
  const calls = stubFetch({
    [`GET ${BASE}/proposals`]: {
      body: {
        proposals: [
          proposalSummary("P-3", {
            title: "Login form validation",
            author: "web-worker",
            human_review: { reason: "auth", reviewer: "tanim", set_by: "main" },
            human_approved: false,
          }),
          proposalSummary("P-4", {
            title: "Login page layout",
            task: "T-11",
            author: "web-styles",
          }),
        ],
      },
    },
    [`GET ${BASE}/workers`]: {
      body: workerList({
        workers: [
          workerProcess("review", { role: "review_lead", area: null }),
          reviewer,
        ],
      }),
    },
    ...extra,
  });
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  client.setQueryData(keys.reviewBatch("demo", shown.id), shown);
  client.setQueryData(keys.session(), session);
  const root = createRootRoute();
  const index = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => <BatchScreen slug="demo" id={shown.id} />,
  });
  const router = createRouter({
    routeTree: root.addChildren([index]),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  await screen.findByRole("heading", { name: /Review batch/ });
  return calls;
}

describe("BatchScreen", () => {
  it("switches between each member's full change and its change since the last review", async () => {
    const user = userEvent.setup();
    const changed = (files: number, added: number) => ({
      body: {
        view: "files",
        source: "recorded",
        from: {
          kind: "revision",
          id: "r17",
          base: null,
          task: null,
          worker: null,
        },
        to: {
          kind: "changeset",
          id: "C-4",
          base: null,
          task: "T-12",
          worker: null,
        },
        files: [],
        totals: { files, added_lines: added, removed_lines: 0 },
        warnings: [],
      },
    });
    await setup(
      batch({
        members: [
          member("P-3", {
            position: 1,
            version: 2,
            current_version: 2,
            since: {
              version: 1,
              totals: { files: 1, added_lines: 3, removed_lines: 1 },
            },
          }),
          member("P-4", { position: 2, task: "T-11" }),
        ],
      }),
      {
        [`GET ${BASE}/proposals/P-3/diff`]: changed(3, 96),
        [`GET ${BASE}/proposals/P-4/diff`]: changed(2, 40),
      },
    );
    const table = screen.getByRole("table", { name: "Members" });
    expect(within(table).getByText("1 file · +3 −1")).toBeTruthy();
    expect(within(table).getByText("since v1")).toBeTruthy();
    expect(await within(table).findByText("2 files · +40 −0")).toBeTruthy();
    expect(
      within(table).getByRole("link", { name: "P-3" }).getAttribute("href"),
    ).toBe("/p/demo/proposals/P-3?since=1");
    const change = screen.getByRole("group", { name: "Change" });
    await user.click(
      within(change).getByRole("button", { name: "Full change" }),
    );
    expect(await within(table).findByText("3 files · +96 −0")).toBeTruthy();
    expect(within(table).queryByText("since v1")).toBeNull();
    expect(
      within(table).getByRole("link", { name: "P-3" }).getAttribute("href"),
    ).toBe("/p/demo/proposals/P-3");
  });

  it("lists the members with their verdicts, the checkout and the reviewer", async () => {
    await setup(rb3);
    const table = screen.getByRole("table", { name: "Members" });
    expect(
      await within(table).findByText("Login form validation"),
    ).toBeTruthy();
    expect(within(table).getByText("v2 of 2")).toBeTruthy();
    expect(within(table).getByText("approved")).toBeTruthy();
    expect(within(table).getByText("reviewing")).toBeTruthy();
    expect(within(table).getByText("@tanim")).toBeTruthy();
    expect(within(table).getByText("@web-styles")).toBeTruthy();
    const checkout = screen.getByRole("region", { name: "Batch checkout" });
    expect(within(checkout).getByText("ready")).toBeTruthy();
    expect(
      within(checkout).getByText("ws-12 · main r17 + P-3 + P-4"),
    ).toBeTruthy();
    expect(within(checkout).getByText(".fold/ws/review-rb-3")).toBeTruthy();
    const card = await screen.findByRole("region", { name: "Reviewer" });
    expect(within(card).getByText("Reviewer · sonnet")).toBeTruthy();
    expect(within(card).getByText(/21 turns/)).toBeTruthy();
    expect(screen.getByText("@review")).toBeTruthy();
    expect(screen.getByText(/2 proposals, one verdict each/)).toBeTruthy();
  });

  it("shows a conflicted member with its banner", async () => {
    await setup(rb3, {
      [`GET ${BASE}/proposals`]: {
        body: { proposals: [proposalSummary("P-4", { conflicted: true })] },
      },
    });
    expect(await screen.findByText("P-4 conflicts with main")).toBeTruthy();
    expect(screen.getByText("conflicts with main")).toBeTruthy();
    expect(
      screen.getByRole("link", { name: "Show conflict" }).getAttribute("href"),
    ).toBe("/p/demo/proposals/P-4");
  });

  it("stops the reviewer and keeps the button pending until the daemon's state shows", async () => {
    let stopped = false;
    await setup(rb3, {
      [`POST ${BASE}/workers/review-b3/stop`]: () => {
        stopped = true;
        return { body: { ...reviewer, state: "stopping" } };
      },
      [`GET ${BASE}/workers`]: () => ({
        body: workerList({
          workers: [{ ...reviewer, state: stopped ? "stopped" : "working" }],
        }),
      }),
    });
    await userEvent.click(
      await screen.findByRole("button", { name: "Stop reviewer" }),
    );
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: /reviewer/ })).toBeNull();
    });
  });

  it("integrates a ready batch on request and keeps the action pending until the daemon answers", async () => {
    const ready = batch({
      ...rb3,
      state: "submitted",
      integration: {
        ...rb3.integration,
        state: "ready",
        members: ["P-3", "P-4"],
      },
    });
    const ended = batch({
      ...ready,
      integration: {
        ...ready.integration,
        state: "ended",
        outcome: "together",
        members: [],
        revisions: [
          { proposal: "P-3", version: 2, revision: "r18" },
          { proposal: "P-4", version: 1, revision: "r19" },
        ],
      },
    });
    let release: () => void = () => undefined;
    const answered = new Promise<void>((resolve) => {
      release = resolve;
    });
    let integrated = false;
    const calls = await setup(ready, {
      [`POST ${BASE}/reviews/batches/rb-3/integrate`]: async () => {
        await answered;
        integrated = true;
        return {
          body: { batch: "rb-3", result: "together", run: null, members: [] },
        };
      },
      [`GET ${BASE}/reviews/batches/rb-3`]: () => ({
        body: integrated ? ended : ready,
      }),
    });
    const region = screen.getByRole("region", { name: "Integration" });
    expect(
      within(region).getByText(
        "Ready: P-3, P-4 integrate together, with one run of the integration checks.",
      ),
    ).toBeTruthy();
    await userEvent.click(
      within(region).getByRole("button", { name: "Integrate the batch" }),
    );
    const pending = await within(region).findByRole("button", {
      name: "Integrating…",
    });
    expect(pending.getAttribute("aria-busy")).toBe("true");
    expect(within(region).queryByText(/Integrated together/)).toBeNull();
    release();
    expect(
      await within(region).findByText(
        "Integrated together: P-3 as r18, P-4 as r19.",
      ),
    ).toBeTruthy();
    expect(within(region).queryByRole("button")).toBeNull();
    expect(
      calls.filter((call) => call.method === "POST").map((call) => call.path),
    ).toEqual([`${BASE}/reviews/batches/rb-3/integrate`]);
  });

  it("shows a refused integrate by its reason and offers it only to a named human", async () => {
    const ready = batch({
      ...rb3,
      state: "submitted",
      integration: { ...rb3.integration, state: "ready", members: ["P-3"] },
    });
    await setup(ready, {
      [`POST ${BASE}/reviews/batches/rb-3/integrate`]: {
        status: 400,
        body: {
          error: {
            code: "invalid_request",
            message: "",
            details: { reason: "batch_waiting" },
          },
        },
      },
    });
    await userEvent.click(
      screen.getByRole("button", { name: "Integrate the batch" }),
    );
    expect((await screen.findByRole("alert")).textContent).toBe(
      "A member has no final verdict yet.",
    );
    cleanup();
    await setup(ready, {}, { human: null, authenticated: true });
    expect(
      screen.queryByRole("button", { name: "Integrate the batch" }),
    ).toBeNull();
  });

  it("offers Stop reviewer only to a named human", async () => {
    await setup(rb3, {}, { human: null, authenticated: true });
    expect(await screen.findByText("Reviewer · sonnet")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Stop reviewer" })).toBeNull();
  });

  it("shows a refused stop with its code", async () => {
    await setup(rb3, {
      [`POST ${BASE}/workers/review-b3/stop`]: errorReply(
        404,
        "worker_process_not_found",
      ),
    });
    await userEvent.click(
      await screen.findByRole("button", { name: "Stop reviewer" }),
    );
    expect((await screen.findByRole("alert")).textContent).toContain(
      "code: worker_process_not_found",
    );
  });
});
