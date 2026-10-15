// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  RouterProvider,
  createMemoryHistory,
  createRouter,
} from "@tanstack/react-router";
import {
  cleanup,
  configure,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  afterAll,
  afterEach,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from "vitest";
import { resetTasks, task as fixtureTask } from "../../../fixtures/tasks";
import { liveStatusStore } from "@/live/status";
import { routeTree } from "@/routeTree.gen";
import { ThemeProvider } from "@/theme/ThemeProvider";
import { threadPath } from "./links";
import { Markdown } from "./Markdown";

// The fixtures answer without the development delay, so the whole app, board included, can be driven test after test.
vi.mock("@/api/fixture", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/api/fixture")>();
  return {
    ...real,
    fromFixture: <M, T>(
      load: (() => Promise<M>) | undefined,
      pick: (module: M) => T,
      empty: T,
    ) => real.fromFixture(load, pick, empty, 0),
  };
});

const WAIT = { timeout: 4_000 };
const TITLE = "Build the login form with validation";

// The board stays mounted beneath the dialog, and the visibility check of every role query over it is slow in jsdom.
beforeAll(() => {
  configure({ defaultHidden: true });
});
afterAll(() => {
  configure({ defaultHidden: false });
});

beforeAll(() => {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
  Element.prototype.scrollIntoView = () => {};
});

beforeEach(() => {
  resetTasks();
});

afterEach(() => {
  cleanup();
  liveStatusStore.reset();
});

function setup(path: string) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const router = createRouter({
    routeTree,
    context: { queryClient },
    history: createMemoryHistory({ initialEntries: [path] }),
  });
  render(
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </ThemeProvider>,
  );
  return router;
}

async function openTask(task = "T-2", title = TITLE) {
  const router = setup(`/p/demo/t/${task}`);
  const dialog = await screen.findByRole("dialog", {}, WAIT);
  await within(dialog).findByRole("heading", { name: title }, WAIT);
  return { router, dialog };
}

/** The fixture's first thread; a message id written out reads as a spacing utility to the token guard. */
function firstThread(): string {
  return fixtureTask("demo", "T-2")?.messages[0]?.thread ?? "";
}

const region = (dialog: HTMLElement, name: string) =>
  within(dialog).getByRole("region", { name });

describe("the task dialog route", () => {
  it("opens from a board card with the board's filters and shows every W03 section", async () => {
    const router = setup("/p/demo/board?label=ui");
    const link = await screen.findByRole("link", { name: TITLE }, WAIT);
    expect(link.getAttribute("href")).toBe("/p/demo/t/T-2?label=ui");
    await userEvent.click(link);
    const dialog = await screen.findByRole("dialog", {}, WAIT);
    await within(dialog).findByRole("heading", { name: TITLE }, WAIT);
    expect(router.state.location.pathname).toBe("/p/demo/t/T-2");
    expect(router.state.location.search).toEqual({ label: "ui" });

    expect(within(dialog).getByText("In Progress")).toBeTruthy();
    expect(
      within(dialog).getByText(
        "Calculated: workspace ws-2 is open, no proposal yet",
      ),
    ).toBeTruthy();
    expect(within(dialog).getByText("is editing")).toBeTruthy();
    expect(
      within(dialog).getByText(
        /4 files · ck-31 3 min ago · base r7 \(main r8\)/,
      ),
    ).toBeTruthy();
    for (const name of [
      "Description",
      "Subtasks",
      "Working changes",
      "Messages",
      "Responsible",
      "Workspace",
      "Checkpoints",
      "Changesets",
      "Proposal",
      "Human review",
      "Dependencies",
      "History",
    ]) {
      expect(region(dialog, name), name).toBeTruthy();
    }
    const changes = region(dialog, "Working changes");
    expect(
      within(changes)
        .getByRole("link", { name: /src\/routes\/login\.tsx/ })
        .getAttribute("href"),
    ).toBe("/p/demo/diff?to=ws-2&path=src%2Froutes%2Flogin.tsx");
    expect(
      within(changes)
        .getByRole("link", { name: "Open diff" })
        .getAttribute("href"),
    ).toBe("/p/demo/diff?to=ws-2");
    expect(
      within(changes)
        .getByRole("link", { name: "Compare with main" })
        .getAttribute("href"),
    ).toBe("/p/demo/diff?from=r8&to=ws-2");
    const messages = region(dialog, "Messages");
    expect(within(messages).getByText("2 linked to T-2")).toBeTruthy();
    expect(
      within(messages)
        .getAllByRole("link", { name: /Open the thread of/ })[0]
        ?.getAttribute("href"),
    ).toBe(threadPath("demo", firstThread()));
    const deps = region(dialog, "Dependencies");
    expect(within(deps).getByText("Form field components")).toBeTruthy();
    expect(within(deps).getByText("Login flow")).toBeTruthy();
  }, 15_000);

  it("closes back to the board with its filters", async () => {
    const router = setup("/p/demo/t/T-2?label=ui");
    const dialog = await screen.findByRole("dialog", {}, WAIT);
    await within(dialog).findByRole("heading", { name: TITLE }, WAIT);
    // A linked item and a dependency open their task over the same filtered board.
    expect(
      within(region(dialog, "Subtasks"))
        .getByRole("link", { name: /Form field components/ })
        .getAttribute("href"),
    ).toBe("/p/demo/t/T-13?label=ui");
    expect(
      within(region(dialog, "Dependencies"))
        .getByRole("link", { name: /Login flow/ })
        .getAttribute("href"),
    ).toBe("/p/demo/t/T-5?label=ui");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Close" }),
    );
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/p/demo/board");
    });
    expect(router.state.location.search).toEqual({ label: "ui" });
    expect(screen.queryByRole("dialog")).toBeNull();
  }, 15_000);

  it("says a task that does not exist is not there", async () => {
    setup("/p/demo/t/T-99");
    const dialog = await screen.findByRole("dialog", {}, WAIT);
    expect(
      await within(dialog).findByRole(
        "heading",
        { name: "T-99 does not exist" },
        WAIT,
      ),
    ).toBeTruthy();
    expect(
      within(dialog).queryByRole("button", { name: "Copy link" }),
    ).toBeNull();
  }, 15_000);
});

describe("creating a task", () => {
  it("opens the create form from the tab row and opens the created task", async () => {
    const router = setup("/p/demo/board?label=ui");
    const nav = await screen.findByRole(
      "navigation",
      { name: "Project" },
      WAIT,
    );
    const link = within(nav).getByRole("link", { name: "New task" });
    expect(link.getAttribute("href")).toBe("/p/demo/t/new");
    await userEvent.click(link);
    const dialog = await screen.findByRole("dialog", {}, WAIT);
    expect(
      within(dialog).getByRole("heading", { name: "New task" }),
    ).toBeTruthy();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create task" }),
    );
    expect(await within(dialog).findByText("Write a title.")).toBeTruthy();
    await userEvent.type(
      within(dialog).getByRole("textbox", { name: "Title" }),
      "Password reset",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create task" }),
    );
    expect(
      within(dialog)
        .getByRole("button", { name: "Create task" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    const opened = await screen.findByRole(
      "heading",
      { name: "Password reset" },
      WAIT,
    );
    expect(opened).toBeTruthy();
    expect(router.state.location.pathname).toBe("/p/demo/t/T-17");
    const shown = screen.getByRole("dialog");
    expect(
      within(shown).getByText(
        "Calculated: every prerequisite is done, nobody has started",
      ),
    ).toBeTruthy();
  }, 15_000);

  it("offers New task on the empty board and adds the task to it", async () => {
    const router = setup("/p/ledger/board");
    const panel = await screen.findByRole(
      "region",
      { name: "No tasks yet" },
      WAIT,
    );
    expect(
      within(panel).getByText(/You can also add one yourself\./),
    ).toBeTruthy();
    await userEvent.click(
      within(panel).getByRole("link", { name: "New task" }),
    );
    const dialog = await screen.findByRole("dialog", {}, WAIT);
    await userEvent.type(
      within(dialog).getByRole("textbox", { name: "Title" }),
      "First task",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create task" }),
    );
    await screen.findByRole("heading", { name: "First task" }, WAIT);
    expect(router.state.location.pathname).toBe("/p/ledger/t/T-1");
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(
      await screen.findByRole("link", { name: "First task" }, WAIT),
    ).toBeTruthy();
  }, 15_000);

  it("closes the create form back to the board", async () => {
    const router = setup("/p/demo/t/new?blocked=true");
    const dialog = await screen.findByRole("dialog", {}, WAIT);
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Cancel" }),
    );
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/p/demo/board");
    });
    expect(router.state.location.search).toEqual({ blocked: true });
  }, 15_000);
});

describe("the human edits", () => {
  it("shows the pending state for a title edit, then the confirmed title", async () => {
    const { dialog } = await openTask();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Edit the title" }),
    );
    const field = within(dialog).getByRole("textbox", { name: "Title" });
    await userEvent.clear(field);
    await userEvent.type(field, "Sign-in form");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Save title" }),
    );
    const save = within(dialog).getByRole("button", { name: "Save title" });
    expect(save.getAttribute("aria-busy")).toBe("true");
    expect(
      within(dialog).queryByRole("heading", { name: "Sign-in form" }),
    ).toBeNull();
    expect(
      await within(dialog).findByRole(
        "heading",
        { name: "Sign-in form" },
        WAIT,
      ),
    ).toBeTruthy();
    expect(
      within(dialog).queryByRole("button", { name: "Save title" }),
    ).toBeNull();
  }, 15_000);

  it("refuses an empty title in the form without a write", async () => {
    const { dialog } = await openTask();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Edit the title" }),
    );
    await userEvent.clear(
      within(dialog).getByRole("textbox", { name: "Title" }),
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Save title" }),
    );
    expect(await within(dialog).findByText("Write a title.")).toBeTruthy();
  }, 15_000);

  it("shows the pending state for a description edit, then the rendered Markdown", async () => {
    const { dialog } = await openTask();
    const section = region(dialog, "Description");
    await userEvent.click(
      within(section).getByRole("button", { name: "Edit the description" }),
    );
    const field = within(section).getByRole("textbox", { name: "Description" });
    await userEvent.clear(field);
    await userEvent.type(field, "Only **this** now");
    await userEvent.click(
      within(section).getByRole("button", { name: "Save description" }),
    );
    expect(
      within(section)
        .getByRole("button", { name: "Save description" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(
      await within(section).findByText("this", { selector: "strong" }, WAIT),
    ).toBeTruthy();
    expect(within(section).queryByRole("table")).toBeNull();
  }, 15_000);

  it("adds a label with a pending state and shows it once confirmed", async () => {
    const { dialog } = await openTask();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Label" }),
    );
    await waitFor(() => {
      expect(screen.getAllByRole("dialog")).toHaveLength(2);
    });
    const popover = screen.getAllByRole("dialog")[1] as HTMLElement;
    await userEvent.type(
      within(popover).getByRole("textbox", { name: "Label name" }),
      "copy",
    );
    await userEvent.selectOptions(
      within(popover).getByRole("combobox", { name: "Colour" }),
      "gold",
    );
    await userEvent.click(
      within(popover).getByRole("button", { name: "Add label" }),
    );
    expect(
      within(popover)
        .getByRole("button", { name: "Add label" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(
      await within(popover).findByRole(
        "button",
        { name: "Remove label copy" },
        WAIT,
      ),
    ).toBeTruthy();
    await userEvent.click(
      within(popover).getByRole("button", { name: "Remove label copy" }),
    );
    await waitFor(() => {
      expect(
        within(popover).queryByRole("button", { name: "Remove label copy" }),
      ).toBeNull();
    }, WAIT);
  }, 15_000);

  it("ticks a plain step only after the daemon confirms it", async () => {
    const { dialog } = await openTask();
    const subtasks = region(dialog, "Subtasks");
    const box = within(subtasks).getByRole("checkbox", {
      name: "Map invalid_credentials and rate_limited to messages",
    });
    expect(box.getAttribute("aria-checked")).toBe("false");
    await userEvent.click(box);
    expect(within(subtasks).getByText("Saving…")).toBeTruthy();
    expect(box.getAttribute("aria-checked")).toBe("false");
    await waitFor(() => {
      expect(
        within(subtasks)
          .getByRole("checkbox", {
            name: "Map invalid_credentials and rate_limited to messages",
          })
          .getAttribute("aria-checked"),
      ).toBe("true");
    }, WAIT);
    expect(within(subtasks).getByText("5/5")).toBeTruthy();
  }, 15_000);

  it("gives a linked item no checkbox and a plain step one", async () => {
    const { dialog } = await openTask();
    const subtasks = region(dialog, "Subtasks");
    expect(within(subtasks).getAllByRole("checkbox")).toHaveLength(4);
    const linked = subtasks.querySelector('[data-item="T-2-5"]');
    expect(linked).not.toBeNull();
    expect(within(linked as HTMLElement).queryByRole("checkbox")).toBeNull();
    expect(
      within(linked as HTMLElement).getByText("Done · ticked by fold"),
    ).toBeTruthy();
    expect(
      within(linked as HTMLElement)
        .getByRole("link", { name: /T-13/ })
        .getAttribute("href"),
    ).toBe("/p/demo/t/T-13");
    const step = subtasks.querySelector('[data-item="T-2-1"]');
    expect(within(step as HTMLElement).getAllByRole("checkbox")).toHaveLength(
      1,
    );
  }, 15_000);

  it("adds a plain step with a pending state", async () => {
    const { dialog } = await openTask();
    const subtasks = region(dialog, "Subtasks");
    await userEvent.click(
      within(subtasks).getByRole("button", {
        name: "Add a step or link a task",
      }),
    );
    await userEvent.type(
      within(subtasks).getByRole("textbox", { name: "Step" }),
      "Write the copy",
    );
    await userEvent.click(
      within(subtasks).getByRole("button", { name: "Add step" }),
    );
    expect(
      within(subtasks)
        .getByRole("button", { name: "Add step" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(
      await within(subtasks).findByRole(
        "checkbox",
        { name: "Write the copy" },
        WAIT,
      ),
    ).toBeTruthy();
  }, 15_000);

  it("says a link adds the dependency, and removing it removes the dependency", async () => {
    const { dialog } = await openTask();
    const subtasks = region(dialog, "Subtasks");
    await userEvent.click(
      within(subtasks).getByRole("button", {
        name: "Add a step or link a task",
      }),
    );
    await userEvent.selectOptions(
      within(subtasks).getByRole("combobox", { name: "Kind" }),
      "linked",
    );
    expect(within(subtasks).getByText(/also adds the dependency/)).toBeTruthy();
    await userEvent.click(
      within(subtasks).getByRole("button", { name: "Cancel" }),
    );

    await userEvent.click(
      within(subtasks).getByRole("button", {
        name: "Actions for T-13 Form field components",
      }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Remove" }),
    );
    const confirm = within(subtasks).getByRole("group", {
      name: "Remove the link to T-13",
    });
    expect(
      within(confirm).getByText(
        "Removing the link also removes the dependency: T-2 will no longer need T-13.",
      ),
    ).toBeTruthy();
    await userEvent.click(
      within(confirm).getByRole("button", { name: "Remove link" }),
    );
    await waitFor(() => {
      expect(subtasks.querySelector('[data-item="T-2-5"]')).toBeNull();
    }, WAIT);
    const needs = within(region(dialog, "Dependencies")).getByRole("list", {
      name: "Needs",
    });
    expect(within(needs).queryByText("Form field components")).toBeNull();
  }, 15_000);

  it("adds a dependency with a pending state, then removes it", async () => {
    const { dialog } = await openTask();
    const deps = region(dialog, "Dependencies");
    await userEvent.click(
      within(deps).getByRole("button", { name: "Add dependency" }),
    );
    await userEvent.type(
      within(deps).getByRole("textbox", { name: "T-2 needs" }),
      "T-16",
    );
    await userEvent.click(
      within(deps).getByRole("button", { name: "Add dependency" }),
    );
    expect(
      within(deps)
        .getByRole("button", { name: "Add dependency" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    const needs = within(deps).getByRole("list", { name: "Needs" });
    expect(
      await within(needs).findByText("Scaffold the web app", {}, WAIT),
    ).toBeTruthy();
    await userEvent.click(
      within(needs).getByRole("button", {
        name: "Remove the dependency on T-16",
      }),
    );
    await waitFor(() => {
      expect(within(needs).queryByText("Scaffold the web app")).toBeNull();
    }, WAIT);
    expect(
      within(needs).queryByRole("button", {
        name: "Remove the dependency on T-13",
      }),
    ).toBeNull();
  }, 15_000);

  it("shows a cycle refusal as task ids and leaves the dependencies unchanged", async () => {
    const { dialog } = await openTask();
    const deps = region(dialog, "Dependencies");
    const needs = within(deps).getByRole("list", { name: "Needs" });
    const before = needs.textContent;
    await userEvent.click(
      within(deps).getByRole("button", { name: "Add dependency" }),
    );
    await userEvent.type(
      within(deps).getByRole("textbox", { name: "T-2 needs" }),
      "T-5",
    );
    await userEvent.click(
      within(deps).getByRole("button", { name: "Add dependency" }),
    );
    const alert = await within(deps).findByRole("alert", {}, WAIT);
    expect(alert.textContent).toBe(
      "The edge would close a cycle: T-2 → T-5 → T-2.",
    );
    expect(within(deps).getByRole("list", { name: "Needs" }).textContent).toBe(
      before,
    );
  }, 15_000);

  it("shows any other refusal in an error banner with the daemon's message", async () => {
    const { dialog } = await openTask();
    const deps = region(dialog, "Dependencies");
    await userEvent.click(
      within(deps).getByRole("button", { name: "Add dependency" }),
    );
    await userEvent.type(
      within(deps).getByRole("textbox", { name: "T-2 needs" }),
      "T-99",
    );
    await userEvent.click(
      within(deps).getByRole("button", { name: "Add dependency" }),
    );
    const alert = await within(deps).findByRole("alert", {}, WAIT);
    expect(alert.textContent).toContain("The change was refused.");
    expect(alert.textContent).toContain("task T-99 does not exist");
  }, 15_000);

  it("shows no dependency editor on a Done task", async () => {
    const { dialog } = await openTask("T-13", "Form field components");
    const deps = region(dialog, "Dependencies");
    expect(
      within(deps).queryByRole("button", { name: "Add dependency" }),
    ).toBeNull();
    expect(
      within(deps).queryByRole("button", { name: /Remove the dependency/ }),
    ).toBeNull();
    expect(within(dialog).getByText("Done", { selector: "span" })).toBeTruthy();
  }, 15_000);

  it("turns human review on with a reviewer and a reason, and off", async () => {
    const { dialog } = await openTask();
    const review = region(dialog, "Human review");
    expect(within(review).getByText("Not required")).toBeTruthy();
    await userEvent.click(
      within(review).getByRole("button", { name: "Require my review" }),
    );
    await waitFor(() => {
      expect(
        within(review).getByRole<HTMLInputElement>("textbox", {
          name: "Reviewer",
        }).value,
      ).toBe("tanim");
    }, WAIT);
    await userEvent.type(
      within(review).getByRole("textbox", { name: "Reason" }),
      "Auth work",
    );
    await userEvent.click(
      within(review).getByRole("button", { name: "Require review" }),
    );
    expect(
      await within(review).findByText("Required · tanim", {}, WAIT),
    ).toBeTruthy();
    expect(within(review).getByText("Auth work")).toBeTruthy();
    expect(within(review).getByText(/Set by human:tanim/)).toBeTruthy();
    await userEvent.click(
      within(review).getByRole("button", { name: "Turn off human review" }),
    );
    expect(
      await within(review).findByText("Not required", {}, WAIT),
    ).toBeTruthy();
  }, 15_000);

  it("sends feedback to the claim holder and shows it once confirmed", async () => {
    const { dialog } = await openTask();
    const messages = region(dialog, "Messages");
    const box = within(messages).getByRole("textbox", { name: "Feedback" });
    expect(box.getAttribute("placeholder")).toBe(
      "Write feedback to frontend/ui…",
    );
    await userEvent.type(box, "Use the shorter copy");
    await userEvent.click(
      within(messages).getByRole("button", { name: "Send" }),
    );
    expect(
      within(messages)
        .getByRole("button", { name: "Send" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(
      await within(messages).findByText("Use the shorter copy", {}, WAIT),
    ).toBeTruthy();
    expect(within(messages).getByText("3 linked to T-2")).toBeTruthy();
  }, 15_000);
});

describe("Markdown", () => {
  it("renders tables and task lists and drops raw HTML", () => {
    const { container } = render(
      <Markdown>
        {[
          "| Code | Message |",
          "| --- | --- |",
          "| `rate_limited` | Try later. |",
          "",
          "- [x] Agree the copy",
          "- [ ] Check it",
          "",
          "Some <b>bold</b> words",
          "",
          "<script>alert(1)</script>",
          "",
          '<div class="x">block</div>',
          "",
          "[run](javascript:alert(2)) and [board](/p/demo/board)",
        ].join("\n")}
      </Markdown>,
    );
    expect(screen.getByRole("table")).toBeTruthy();
    expect(
      screen.getAllByRole("columnheader").map((h) => h.textContent),
    ).toEqual(["Code", "Message"]);
    const boxes = screen.getAllByRole<HTMLInputElement>("checkbox");
    expect(boxes.map((b) => b.checked)).toEqual([true, false]);
    expect(boxes.every((b) => b.disabled)).toBe(true);
    expect(container.querySelector("b, script, div.x")).toBeNull();
    expect(container.textContent).not.toContain("<b>");
    expect(container.textContent).not.toContain("alert(1)");
    expect(container.textContent).not.toContain("block");
    expect(
      [...container.querySelectorAll("a")].map((a) => a.getAttribute("href")),
    ).toEqual(["", "/p/demo/board"]);
  });
});
