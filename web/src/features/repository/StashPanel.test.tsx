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

function entry(stash: string, files: string[], conflicted: string[] = []) {
  return {
    stash,
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
      calls.filter((call) => call.method === "DELETE").map((call) => call.path),
    ).toEqual(["/api/v1/projects/demo/stash/1"]);
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

  it("clears all after the question and shows a refusal", async () => {
    stubFetch({
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
      await screen.findByText(/waits in the project folder: a/),
    ).toBeTruthy();
  });
});
