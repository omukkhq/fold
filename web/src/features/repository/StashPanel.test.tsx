// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { stubFetch } from "../../test/stubFetch";
import { StashPanel } from "./StashPanel";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const NOW = new Date(2026, 10, 7, 15, 0).getTime();

/** An entry whose number follows from its place, 100 for `stash@{0}`, unless a test gives another. */
function entry(stash: string, files: string[], conflicted: string[] = []) {
  return {
    stash,
    number: 100 - Number(/\d+/.exec(stash)?.[0] ?? 0),
    created_at: new Date(2026, 10, 7, 14, 12).getTime(),
    base: "r8",
    actor: "human:ana",
    files,
    conflicted,
  };
}

function show() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <StashPanel slug="demo" now={NOW} />
    </QueryClientProvider>,
  );
}

describe("StashPanel", () => {
  it("renders nothing for an empty stash", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/stash": { body: { entries: [] } },
    });
    show();
    await waitFor(() => {
      expect(calls.length).toBe(1);
    });
    expect(screen.queryByRole("region", { name: "Stash" })).toBeNull();
  });

  it("asks before it drops an entry and stays pending until the stash is read again", async () => {
    let listed = [
      entry("stash@{0}", ["notes.md"]),
      entry("stash@{1}", ["a", "b"]),
    ];
    let release: () => void = () => undefined;
    const calls = stubFetch({
      "GET /api/v1/projects/demo/stash": () => ({ body: { entries: listed } }),
      "DELETE /api/v1/projects/demo/stash/1": () =>
        new Promise((resolve) => {
          release = () => {
            listed = [entry("stash@{0}", ["notes.md"])];
            resolve({ body: { dropped: [], entries: 1 } });
          };
        }),
    });
    show();
    fireEvent.click(
      await screen.findByRole("button", { name: "Drop stash@{1}" }),
    );
    expect(screen.getByText("Drop stash@{1}?")).toBeTruthy();
    // No error yet, so the dialog draws no band between its words and its buttons.
    expect(
      screen.getByRole("dialog").querySelector(".overflow-y-auto"),
    ).toBeNull();
    expect(
      screen.getByText(/2 files in stash@\{1\} are gone for good/),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Drop entry" }));
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /Dropping/ })).toBeTruthy();
    });
    release();
    await waitFor(() => {
      expect(screen.queryByText("Drop stash@{1}?")).toBeNull();
    });
    expect(screen.queryByRole("button", { name: "Drop stash@{1}" })).toBeNull();
    expect(
      calls
        .filter((call) => call.method === "DELETE")
        .map((call) => `${call.path}?${call.search.toString()}`),
    ).toEqual(["/api/v1/projects/demo/stash/1?number=99"]);
  });

  it("clears every entry, and has no drop while an unstash conflict waits", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/stash": {
        body: {
          entries: [
            entry("stash@{0}", ["plan.md"], ["plan.md"]),
            entry("stash@{1}", ["a"]),
          ],
        },
      },
    });
    show();
    await screen.findByText("Stash");
    expect(screen.queryByRole("button", { name: "Drop stash@{0}" })).toBeNull();
    expect(screen.getByRole("button", { name: "Drop stash@{1}" })).toBeTruthy();
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Clear all" })
        .disabled,
    ).toBe(true);
    expect(calls.length).toBe(1);
  });

  it("clears all after the question, naming the stash it saw, and shows a refusal", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/stash": {
        body: {
          entries: [entry("stash@{0}", ["a"]), entry("stash@{1}", ["b"])],
        },
      },
      "DELETE /api/v1/projects/demo/stash": {
        status: 400,
        body: {
          error: {
            code: "invalid_request",
            message:
              "the conflict of fld unstash waits in the project folder: a",
            details: { reason: "stash_conflicted" },
          },
        },
      },
    });
    show();
    fireEvent.click(await screen.findByRole("button", { name: "Clear all" }));
    expect(screen.getByText("Drop all 2 stash entries?")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Drop all" }));
    expect(
      await screen.findByText(
        /An unstash conflict waits in the project folder\. Fix the files/,
      ),
    ).toBeTruthy();
    expect(
      calls
        .filter((call) => call.method === "DELETE")
        .map((call) => call.search.toString()),
    ).toEqual(["entries=2&newest=100"]);
  });

  it("refuses a drop when a newer entry took the confirmed entry's place, and shows the stash read again", async () => {
    const junk = entry("stash@{0}", ["junk.txt"]);
    let listed = [junk];
    const calls = stubFetch({
      "GET /api/v1/projects/demo/stash": () => ({ body: { entries: listed } }),
    });
    show();
    fireEvent.click(
      await screen.findByRole("button", { name: "Drop stash@{0}" }),
    );
    listed = [
      { ...entry("stash@{0}", ["plan.md"]), number: junk.number + 1 },
      { ...junk, stash: "stash@{1}" },
    ];
    fireEvent.click(screen.getByRole("button", { name: "Drop entry" }));
    expect(
      await screen.findByText("The stash changed; check the list again."),
    ).toBeTruthy();
    // The dialog hides the list from the accessibility tree while it is open.
    expect(
      await screen.findByRole("button", {
        name: "Drop stash@{1}",
        hidden: true,
      }),
    ).toBeTruthy();
    expect(calls.some((call) => call.method === "DELETE")).toBe(false);
    expect(screen.queryByRole("button", { name: "Drop entry" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Back to the list" }));
    await waitFor(() => {
      expect(screen.queryByText("Drop stash@{0}?")).toBeNull();
    });
    expect(screen.getByRole("button", { name: "Drop stash@{1}" })).toBeTruthy();
  });

  it("refuses Clear all when the stash grew after the person confirmed it", async () => {
    let listed = [entry("stash@{0}", ["a"])];
    const calls = stubFetch({
      "GET /api/v1/projects/demo/stash": () => ({ body: { entries: listed } }),
    });
    show();
    fireEvent.click(await screen.findByRole("button", { name: "Clear all" }));
    listed = [
      { ...entry("stash@{0}", ["b"]), number: 1 },
      entry("stash@{1}", ["a"]),
    ];
    fireEvent.click(screen.getByRole("button", { name: "Drop all" }));
    expect(
      await screen.findByText("The stash changed; check the list again."),
    ).toBeTruthy();
    expect(calls.some((call) => call.method === "DELETE")).toBe(false);
    expect(screen.queryByRole("button", { name: "Drop all" })).toBeNull();
    expect(
      screen.getByRole("button", { name: "Back to the list" }),
    ).toBeTruthy();
  });

  it("says the stash changed when the daemon refuses a drop with stash_changed", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/stash": {
        body: { entries: [entry("stash@{0}", ["a"])] },
      },
      "DELETE /api/v1/projects/demo/stash/0": {
        status: 400,
        body: {
          error: {
            code: "invalid_request",
            message: "the stash changed",
            details: { reason: "stash_changed" },
          },
        },
      },
    });
    show();
    fireEvent.click(
      await screen.findByRole("button", { name: "Drop stash@{0}" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Drop entry" }));
    expect(
      await screen.findByText("The stash changed; check the list again."),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Drop entry" })).toBeNull();
    expect(
      screen.getByRole("button", { name: "Back to the list" }),
    ).toBeTruthy();
  });

  it("shows an error when the stash cannot be read", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/stash": {
        status: 503,
        body: {
          error: { code: "project_unavailable", message: "closed" },
        },
      },
    });
    show();
    expect(
      await screen.findByText("fold could not read the stash."),
    ).toBeTruthy();
    expect(screen.getByText("The project is not available now.")).toBeTruthy();
  });

  it("puts focus on the entry that took the dropped entry's place", async () => {
    let listed = [
      entry("stash@{0}", ["notes.md"]),
      entry("stash@{1}", ["a"]),
      { ...entry("stash@{2}", ["b"]), number: 1 },
    ];
    stubFetch({
      "GET /api/v1/projects/demo/stash": () => ({ body: { entries: listed } }),
      "DELETE /api/v1/projects/demo/stash/1": () => {
        listed = [
          entry("stash@{0}", ["notes.md"]),
          { ...entry("stash@{1}", ["b"]), number: 1 },
        ];
        return { body: { dropped: [], entries: 2 } };
      },
    });
    show();
    fireEvent.click(
      await screen.findByRole("button", { name: "Drop stash@{1}" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Drop entry" }));
    await waitFor(() => {
      expect(screen.queryByText("Drop stash@{1}?")).toBeNull();
    });
    await waitFor(() => {
      expect(document.activeElement?.getAttribute("aria-label")).toBe(
        "Drop stash@{1}",
      );
    });
  });
});
