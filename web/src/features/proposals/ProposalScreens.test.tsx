// @vitest-environment jsdom
import { shellRoutes, stubFetch } from "@/test/stubFetch";
import { QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, createMemoryHistory } from "@tanstack/react-router";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "@/api/errors";
import { createQueryClient } from "@/app/queryClient";
import { createAppRouter } from "@/app/router";
import { ThemeProvider } from "@/theme/ThemeProvider";
import { detail, reset } from "../../../fixtures/proposals";

const failures = vi.hoisted(() => ({ next: null as Error | null }));
const gate = vi.hoisted(() => ({ wait: null as Promise<void> | null }));

// The fixtures answer after a delay so that pending states show in development; these tests answer at once, or when a test lets a held answer go.
vi.mock("@/api/fixture", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/fixture")>();
  return {
    ...actual,
    fromFixture: async <M, T>(
      load: (() => Promise<M>) | undefined,
      pick: (module: M) => T,
      empty: T,
    ) => {
      if (gate.wait) await gate.wait;
      return actual.fromFixture(load, pick, empty, 0);
    },
  };
});

/** Holds every fixture answer until the returned function runs, so a test can see a write pending. */
function hold(): () => void {
  let release: () => void = () => undefined;
  gate.wait = new Promise<void>((resolve) => {
    release = resolve;
  });
  return () => {
    gate.wait = null;
    release();
  };
}

// A write can be made to fail once, with a code the screen does not know.
vi.mock("../../../fixtures/proposals", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("../../../fixtures/proposals")>();
  const failing =
    <A extends unknown[], R>(write: (...args: A) => R) =>
    (...args: A): R => {
      const failure = failures.next;
      if (failure) {
        failures.next = null;
        throw failure;
      }
      return write(...args);
    };
  return {
    ...actual,
    review: failing(actual.review),
    integrate: failing(actual.integrate),
  };
});

beforeEach(() => {
  stubFetch(shellRoutes(["demo", "ledger"]));
  reset();
  failures.next = null;
  gate.wait = null;
  vi.spyOn(window, "scrollTo").mockImplementation(() => undefined);
});
afterEach(() => {
  vi.unstubAllGlobals();
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
  return { router, container: view.container };
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
    expect(cells.getByText("backend/db")).toBeTruthy();
    expect(cells.getByText("v2 of 2")).toBeTruthy();
    expect(cells.getByRole("img", { name: "human" })).toBeTruthy();
    expect(cells.getByText("Agent approved · needs tanim")).toBeTruthy();
    expect(cells.getByText("claimed by review/quality")).toBeTruthy();
    expect(cells.getByText("Human review · tanim")).toBeTruthy();
    expect(
      screen
        .getByRole("link", { name: "Open · 3" })
        .getAttribute("aria-current"),
    ).toBe("page");
  });

  it("shows integrated and closed proposals under their segments", async () => {
    const user = userEvent.setup();
    renderAt("/p/demo/proposals");
    await table();
    await user.click(screen.getByRole("link", { name: "Integrated" }));
    await waitFor(async () => {
      expect(rowIds(await table())).toEqual(["P-5", "P-7", "P-2", "P-1"]);
    });
    expect(screen.getByText("Integrated as r8")).toBeTruthy();
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
    expect(
      screen
        .getByRole("button", { name: "Conflicted" })
        .getAttribute("aria-pressed"),
    ).toBe("true");
    await user.click(screen.getByRole("button", { name: "Claimed" }));
    await user.click(
      await screen.findByRole("menuitem", { name: "Unclaimed" }),
    );
    expect(
      await screen.findByRole("region", { name: "No proposal matches" }),
    ).toBeTruthy();
  });

  it("shows the empty state for a project without proposals", async () => {
    renderAt("/p/ledger/proposals");
    expect(
      await screen.findByRole("region", { name: "No proposals yet" }),
    ).toBeTruthy();
  });
});

describe("Proposal detail", () => {
  it("switches versions; a v1 comment is outdated on v2 and current on v1, and an old version takes no verdict", async () => {
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4");
    const thread = await screen.findByRole("list", { name: "Thread rc-1" });
    expect(within(thread).getByText("on v1 · line 10 · outdated")).toBeTruthy();
    expect(within(thread).getByText("reply · v2")).toBeTruthy();
    expect(
      screen
        .getByRole("link", { name: "v2 · approved" })
        .getAttribute("aria-current"),
    ).toBe("page");

    await user.click(
      screen.getByRole("link", { name: "v1 · changes requested" }),
    );
    expect(await screen.findByText("on v1 · line 10")).toBeTruthy();
    expect(screen.queryByText(/outdated/)).toBeNull();
    expect(screen.getByText(/v1 is an earlier version/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Approve v2" })).toBeNull();
    expect(screen.getByRole("region", { name: "Version v1" })).toBeTruthy();
    expect(screen.getByText(/46 passed/)).toBeTruthy();

    await user.click(screen.getByRole("link", { name: "Compare v1 → v2" }));
    expect(
      await screen.findByRole("link", {
        name: "Compare v1 → v2",
        current: "page",
      }),
    ).toBeTruthy();
    await waitFor(() => {
      expect(screen.queryByRole("list", { name: "Thread rc-1" })).toBeNull();
    });
  });

  it("opens the composer on one line and on a run of lines, and saves the comment with its range and side after the fixture answers", async () => {
    const user = userEvent.setup();
    const { container } = renderAt("/p/demo/proposals/P-4");
    await user.click(await lineCell(container, "new:3"));
    expect(
      await screen.findByRole("form", {
        name: "Comment on line 3 · new side · tanim",
      }),
    ).toBeTruthy();

    await user.click(await lineCell(container, "new:2"));
    fireEvent.click(await lineCell(container, "new:4"), { shiftKey: true });
    const form = await screen.findByRole("form", {
      name: "Comment on lines 2–4 · new side · tanim",
    });
    await user.type(
      within(form).getByLabelText("Comment"),
      "owner_id and report_date need defaults.",
    );
    await user.click(
      within(form).getByRole("button", { name: "Add to review" }),
    );
    expect(
      within(form)
        .getByRole("button", { name: "Add to review" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(
      screen.queryByText("owner_id and report_date need defaults."),
    ).toBeNull();

    expect(
      await screen.findByText("owner_id and report_date need defaults."),
    ).toBeTruthy();
    expect(screen.getByText("on v2 · lines 2–4")).toBeTruthy();
    const reviews = screen.getByRole("region", { name: "Reviews" });
    expect(within(reviews).queryByText("Comment")).toBeNull();
    expect(detail("demo", "P-4")?.comments.at(-1)).toMatchObject({
      version: 2,
      path: "migrations/0004_reports.sql",
      side: "new",
      start_line: 2,
      end_line: 4,
    });
  });

  it("asks for a body before it sends a comment", async () => {
    const user = userEvent.setup();
    const { container } = renderAt("/p/demo/proposals/P-4");
    await user.click(await lineCell(container, "new:3"));
    const form = await screen.findByRole("form", { name: /Comment on line 3/ });
    await user.click(
      within(form).getByRole("button", { name: "Add to review" }),
    );
    expect(
      await within(form).findByText("Write a comment first."),
    ).toBeTruthy();
    await user.click(within(form).getByRole("button", { name: "Cancel" }));
    expect(
      screen.queryByRole("form", { name: /Comment on line 3/ }),
    ).toBeNull();
  });

  it("shows Approve as pending, then the human approval once the fixture answers", async () => {
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4");
    const panel = await integration();
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
    expect(within(reviews).getByText(/^tanim · v2/)).toBeTruthy();
    expect(within(panel).getByText(/Approved by tanim/)).toBeTruthy();
  });

  it("needs a body for Request changes, shows it pending, then the verdict once the fixture answers", async () => {
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
    expect(
      within(reviews).queryByText("Requested changes: Rename the index"),
    ).toBeNull();
    release();
    expect(
      await within(reviews).findByText("Requested changes: Rename the index"),
    ).toBeTruthy();
    expect(
      screen.getByText("Changes requested · back with the author"),
    ).toBeTruthy();
  });

  it("shows Integrate after a request for changes, pending, then the new main revision once the fixture answers, and no Reject", async () => {
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
    expect(
      within(panel).queryByRole("button", { name: "Integrate" }),
    ).toBeNull();
  });

  it("shows no Integrate when the payload refuses it", async () => {
    renderAt("/p/demo/proposals/P-6");
    const panel = await integration();
    expect(
      within(panel).getByRole("button", { name: "Approve v1" }),
    ).toBeTruthy();
    expect(
      within(panel).queryByRole("button", { name: "Integrate" }),
    ).toBeNull();
  });

  it("renders the conflict report with every conflict kind and the author's next step", async () => {
    renderAt("/p/demo/proposals/P-6");
    const report = await screen.findByRole("region", {
      name: "Conflict report",
    });
    const kinds = [...report.querySelectorAll("[data-kind]")].map((el) =>
      el.getAttribute("data-kind"),
    );
    expect(kinds.sort()).toEqual([
      "add_add",
      "both_modified_binary",
      "both_modified_text",
      "modify_delete",
      "type_change",
    ]);
    expect(within(report).getAllByText(/^tree [0-9a-f]{7}$/)).toHaveLength(3);
    expect(
      within(report).getByText(/const max = Math\.max\(\.\.\.totals, 1\)/),
    ).toBeTruthy();
    expect(
      within(report).getByText(/frontend\/charts runs sync_workspace/),
    ).toBeTruthy();
    expect(
      screen.getByText("Conflict with main r8 · back with the author"),
    ).toBeTruthy();
    expect(
      await screen.findByRole("heading", { level: 2, name: "Files" }),
    ).toBeTruthy();
  });

  it("shows gates met, unmet and waiting on a human", async () => {
    renderAt("/p/demo/proposals/P-4");
    const panel = await integration();
    expect(
      within(panel).getAllByRole("img", { name: "met" }).length,
    ).toBeGreaterThan(0);
    expect(
      within(panel).getByRole("img", { name: "waiting on a human" }),
    ).toBeTruthy();
    expect(within(panel).getByText(/Waiting for tanim/)).toBeTruthy();
    cleanup();
    renderAt("/p/demo/proposals/P-6");
    const conflicted = await integration();
    expect(
      within(conflicted).getByRole("img", { name: "not met" }),
    ).toBeTruthy();
    expect(within(conflicted).getByText(/Does not merge onto r8/)).toBeTruthy();
  });

  it("shows a known failure under the actions and an unknown one in the error banner", async () => {
    const user = userEvent.setup();
    renderAt("/p/demo/proposals/P-4");
    const panel = await integration();
    failures.next = new ApiError(
      "version_superseded",
      "P-4 v2 is not the current version",
    );
    await user.click(within(panel).getByRole("button", { name: "Approve v2" }));
    expect(
      await within(panel).findByText(/A newer version arrived/),
    ).toBeTruthy();
    failures.next = new ApiError("disk_full", "The disk is full.");
    await user.click(within(panel).getByRole("button", { name: "Integrate" }));
    const banner = await screen.findByText("The disk is full.");
    expect(banner.closest("[role=alert]")?.textContent).toMatch(
      /The request failed/,
    );
    expect(within(panel).queryByText(/A newer version arrived/)).toBeNull();
  });

  it("says when the project has no such proposal", async () => {
    renderAt("/p/demo/proposals/P-99");
    expect(
      await screen.findByRole("region", { name: "No proposal P-99" }),
    ).toBeTruthy();
  });
});
