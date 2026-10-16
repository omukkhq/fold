// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DurableEvent } from "../../api/events";
import type { ReviewCheckout } from "../../api/reviews";
import { applyEvent } from "../../live/apply";
import { liveStatusStore } from "../../live/status";
import { EventStream } from "./EventStream";
import { ReviewCheckouts } from "./ReviewCheckouts";

const listEvents = vi.hoisted(() => vi.fn<() => Promise<DurableEvent[]>>());
const listReviewCheckouts = vi.hoisted(() =>
  vi.fn<() => Promise<ReviewCheckout[]>>(),
);
vi.mock("../../api/events", () => ({ listEvents }));
vi.mock("../../api/reviews", () => ({ listReviewCheckouts }));

const T = new Date(2026, 9, 1, 14, 12, 31).getTime();

function stored(seq: number, over: Partial<DurableEvent> = {}): DurableEvent {
  return {
    seq,
    type: "task.created",
    actor: "fold",
    task: `T-${String(seq)}`,
    proposal: null,
    workspace: null,
    revision: null,
    data: {},
    created_at: T + seq * 1000,
    ...over,
  };
}

let client: QueryClient;

function wrap(node: React.ReactNode) {
  render(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
}

beforeEach(() => {
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  liveStatusStore.reset();
});
afterEach(() => {
  cleanup();
  listEvents.mockReset();
  listReviewCheckouts.mockReset();
});

describe("EventStream", () => {
  it("lists the stored events newest first", async () => {
    listEvents.mockResolvedValue([stored(2), stored(1)]);
    wrap(<EventStream slug="demo" />);
    const items = await screen.findAllByRole("listitem");
    expect(items.map((item) => item.textContent)).toEqual([
      expect.stringContaining("created T-2"),
      expect.stringContaining("created T-1"),
    ]);
    expect(within(items[0] ?? document.body).getByText("@fold")).toBeTruthy();
  });

  it("shows a pending state, then the empty state for a project with no events", async () => {
    let resolve: (value: DurableEvent[]) => void = () => {};
    listEvents.mockReturnValue(
      new Promise((r) => {
        resolve = r;
      }),
    );
    wrap(<EventStream slug="demo" />);
    expect(await screen.findByRole("status")).toBeTruthy();
    resolve([]);
    expect(await screen.findByText(/No events yet/)).toBeTruthy();
  });

  it("shows an error with a retry", async () => {
    listEvents.mockRejectedValueOnce(new Error("down"));
    wrap(<EventStream slug="demo" />);
    await screen.findByRole("alert");
    listEvents.mockResolvedValue([stored(1)]);
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("created T-1")).toBeTruthy();
  });

  it("puts a new stored event from the stream at the top with no reload", async () => {
    listEvents.mockResolvedValue([stored(2), stored(1)]);
    wrap(<EventStream slug="demo" />);
    await screen.findAllByRole("listitem");
    act(() => {
      applyEvent(client, "demo", {
        kind: "event",
        ...stored(3, {
          type: "claim.acquired",
          actor: "frontend/ui",
          task: "T-3",
        }),
      });
    });
    await waitFor(() => {
      expect(screen.getAllByRole("listitem")).toHaveLength(3);
    });
    expect(screen.getAllByRole("listitem")[0]?.textContent).toContain(
      "claimed T-3",
    );
    expect(listEvents).toHaveBeenCalledTimes(1);
  });

  it("follows the connection with its dot", async () => {
    listEvents.mockResolvedValue([]);
    wrap(<EventStream slug="demo" />);
    expect(await screen.findByText("connecting")).toBeTruthy();
    act(() => {
      liveStatusStore.set({ state: "live" });
    });
    expect(await screen.findByText("live")).toBeTruthy();
    act(() => {
      liveStatusStore.set({ state: "reconnecting" });
    });
    expect(await screen.findByText("reconnecting")).toBeTruthy();
  });
});

const checkout = (over: Partial<ReviewCheckout> = {}): ReviewCheckout => ({
  workspace: "ws-12",
  path: "ws/review-p-4",
  target: { kind: "proposal", proposal: "P-4", version: 2 },
  worker: "review/quality",
  base: "r7",
  created_at: Date.now(),
  ...over,
});

describe("ReviewCheckouts", () => {
  it("shows the worker, the checkout and the not recordable mark", async () => {
    listReviewCheckouts.mockResolvedValue([checkout()]);
    wrap(<ReviewCheckouts slug="demo" />);
    await screen.findByText("@review/quality");
    const row = screen.getByRole("region", { name: "Review checkouts" });
    expect(within(row).getByText("@review/quality")).toBeTruthy();
    expect(row.textContent).toContain(
      "P-4 v2 in ws/review-p-4 · not recordable",
    );
  });

  it("shows a batch checkout as one row with its batch and members", async () => {
    listReviewCheckouts.mockResolvedValue([
      checkout({
        workspace: "ws-15",
        path: "ws/review-rb-3",
        worker: "review/batch",
        target: {
          kind: "batch",
          batch: "rb-3",
          members: [
            { proposal: "P-2", version: 1 },
            { proposal: "P-3", version: 1 },
          ],
        },
      }),
    ]);
    wrap(<ReviewCheckouts slug="demo" />);
    await screen.findByText("@review/batch");
    expect(
      screen.getByText(
        "rb-3 · P-2 v1 + P-3 v1 in ws/review-rb-3 · not recordable",
      ),
    ).toBeTruthy();
  });

  it("keeps one title for several checkouts", async () => {
    listReviewCheckouts.mockResolvedValue([
      checkout(),
      checkout({ workspace: "ws-13", worker: "review/security" }),
      checkout({ workspace: "ws-14", worker: null }),
    ]);
    wrap(<ReviewCheckouts slug="demo" />);
    await screen.findByText("@review/security");
    expect(screen.getAllByText("Review checkouts")).toHaveLength(3);
    expect(
      screen.getAllByText("P-4 v2 in ws/review-p-4 · not recordable"),
    ).toHaveLength(3);
    expect(screen.getByText("no worker")).toBeTruthy();
  });

  it("shows the empty state, the pending state and an error", async () => {
    let resolve: (value: ReviewCheckout[]) => void = () => {};
    listReviewCheckouts.mockReturnValueOnce(
      new Promise((r) => {
        resolve = r;
      }),
    );
    wrap(<ReviewCheckouts slug="demo" />);
    expect(await screen.findByRole("status")).toBeTruthy();
    resolve([]);
    expect(await screen.findByText("No review checkout is open.")).toBeTruthy();
    cleanup();
    client.clear();
    listReviewCheckouts.mockRejectedValueOnce(new Error("down"));
    wrap(<ReviewCheckouts slug="demo" />);
    await screen.findByRole("alert");
    listReviewCheckouts.mockResolvedValue([checkout()]);
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("@review/quality")).toBeTruthy();
  });
});
