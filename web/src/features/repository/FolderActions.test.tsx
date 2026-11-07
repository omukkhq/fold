// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { needsItem } from "@/test/workers";
import { stubFetch } from "../../test/stubFetch";
import { FolderActions } from "./FolderActions";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const RESULT = {
  revision: "r4",
  at_head: true,
  files: ["plan.md"],
  stashed: true,
  entries: 1,
  unchanged: [],
  dropped_unstash: [],
  dropped_change: null,
  skipped: [],
  unstash: null,
};

function show() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <FolderActions slug="demo" />
    </QueryClientProvider>,
  );
}

describe("FolderActions", () => {
  it("renders nothing while nothing waits on the project folder", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/needs-you": { body: { items: [] } },
    });
    show();
    await waitFor(() => {
      expect(calls.length).toBe(1);
    });
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("drops the waiting change after the question, which names the safety stash, and stays pending until Needs you is read again", async () => {
    let items = [needsItem({ kind: "human_change", paths: ["plan.md"] })];
    let release: () => void = () => undefined;
    const calls = stubFetch({
      "GET /api/v1/projects/demo/needs-you": () => ({ body: { items } }),
      "POST /api/v1/projects/demo/folder/reset": () =>
        new Promise((resolve) => {
          release = () => {
            items = [];
            resolve({ body: RESULT });
          };
        }),
    });
    show();
    fireEvent.click(
      await screen.findByRole("button", { name: "Drop my change" }),
    );
    expect(screen.getByText("Drop your waiting change?")).toBeTruthy();
    expect(
      screen.getByText(/stashes your unrecorded changes as stash@\{0\}/),
    ).toBeTruthy();
    expect(screen.getByText("plan.md")).toBeTruthy();
    expect(screen.getByText("and your other unrecorded changes")).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: "Restore held files" }),
    ).toBeNull();
    const confirm = screen
      .getAllByRole("button", { name: "Drop my change" })
      .at(-1);
    if (!confirm) throw new Error("the confirm button");
    fireEvent.click(confirm);
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /Resetting/ })).toBeTruthy();
    });
    release();
    await waitFor(() => {
      expect(screen.queryByText("Drop your waiting change?")).toBeNull();
    });
    expect(screen.queryByRole("button", { name: "Drop my change" })).toBeNull();
    expect(calls.filter((call) => call.method === "POST")).toHaveLength(1);
  });

  it("restores the held files and shows a refusal", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/needs-you": {
        body: {
          items: [needsItem({ kind: "folder_held", paths: ["a.md", "b.md"] })],
        },
      },
      "POST /api/v1/projects/demo/folder/restore": {
        status: 400,
        body: {
          error: {
            code: "invalid_request",
            message: "files changed while fold stashed them",
            details: { reason: "changed_during_stash" },
          },
        },
      },
    });
    show();
    expect(
      await screen.findByRole("button", { name: "Reset folder" }),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Restore held files" }));
    expect(screen.getByText("Restore 2 files?")).toBeTruthy();
    expect(screen.getByText("b.md")).toBeTruthy();
    expect(screen.queryByText("and your other unrecorded changes")).toBeNull();
    expect(screen.queryByText(/workspace/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Restore files" }));
    expect(
      await screen.findByText(
        "Files changed while fold stashed them. Nothing was put back; try again.",
      ),
    ).toBeTruthy();
    expect(calls.find((call) => call.method === "POST")?.body).toEqual({
      paths: ["a.md", "b.md"],
    });
  });

  it("names a waiting unstash conflict with its next steps, and puts the dialog's main action last for the shared action row", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/needs-you": {
        body: {
          items: [needsItem({ kind: "unstash_conflicted", paths: ["a.md"] })],
        },
      },
    });
    show();
    const note = await screen.findByText(/An unstash conflict waits in 1 file/);
    expect(note.textContent).toBe(
      "An unstash conflict waits in 1 file of the project folder. Fix the conflict markers and run fld record, or reset the folder.",
    );
    const command = within(note).getByText("fld record");
    expect(command.tagName).toBe("CODE");
    // The note sits on the sunken surface, so its command chip takes the plain surface to stay visible.
    expect(command.className).toContain("bg-surface");
    expect(command.className).toContain("whitespace-nowrap");
    expect(
      screen.queryByRole("button", { name: "Restore held files" }),
    ).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Reset folder" }));
    const buttons = within(screen.getByRole("dialog"))
      .getAllByRole("button")
      .map((button) => button.textContent);
    expect(buttons.slice(-2)).toEqual(["Cancel", "Reset folder"]);
  });
});
