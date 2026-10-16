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
  role: "batch_reviewer",
  area: null,
  model: "sonnet",
  turns: 21,
});

async function setup(
  shown: ReviewBatch,
  extra: Record<string, StubReply | (() => StubReply)> = {},
  session: Session = { human: "tanim", authenticated: true },
) {
  const calls = stubFetch({
    [`GET ${BASE}/proposals`]: {
      body: {
        proposals: [
          proposalSummary("P-3", {
            title: "Login form validation",
            author: "web-coder",
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
    expect(within(card).getByText("Batch reviewer · sonnet")).toBeTruthy();
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

  it("offers Stop reviewer only to a named human", async () => {
    await setup(rb3, {}, { human: null, authenticated: true });
    expect(await screen.findByText("Batch reviewer · sonnet")).toBeTruthy();
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
