// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { stubFetch } from "../../test/stubFetch";
import { GitStatus } from "./GitStatus";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function show() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <GitStatus slug="demo" />
    </QueryClientProvider>,
  );
}

describe("GitStatus", () => {
  it("shows the repository, the branch and the command to add a remote", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/git": {
        body: { repository: "/home/ana/ledger", branch: "main", remotes: [] },
      },
    });
    show();
    expect(await screen.findByText("/home/ana/ledger")).toBeTruthy();
    const command = screen.getByText("fld remote add <name> <url>");
    expect(command.tagName).toBe("CODE");
    expect(command.className).toContain("text-13");
  });

  it("shows each remote with its last push, a failure and the commit", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/git": {
        body: {
          repository: "/home/ana/ledger",
          branch: "main",
          remotes: [
            {
              name: "backup",
              location: "/mnt/backup/ledger.git",
              branch: "trunk",
              push_on_integrate: false,
              pushed: { revision: "r8", commit: "4f2a9c1e4f2a9c1e", at: 1 },
              failed: { code: "remote_ahead", at: 2 },
            },
          ],
        },
      },
    });
    show();
    expect(
      await screen.findByText("push failed: remote has newer commits"),
    ).toBeTruthy();
    expect(screen.getByText("/mnt/backup/ledger.git")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Copy commit 4f2a9c1" }),
    ).toBeTruthy();
    expect(screen.queryByText("No remote. Add one:")).toBeNull();
  });

  it("shows nothing until the read arrives", () => {
    stubFetch({ "GET /api/v1/projects/demo/git": { body: {} } });
    show();
    expect(screen.queryByLabelText("Git status")).toBeNull();
  });
});
