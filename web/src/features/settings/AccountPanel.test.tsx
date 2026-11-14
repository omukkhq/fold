// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ProjectAccount } from "@/api/workers";
import { errorReply, stubFetch } from "@/test/stubFetch";
import { workerList } from "@/test/workers";
import { AccountPanel } from "./AccountPanel";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const WORKERS = "GET /api/v1/projects/demo/workers";
const CHOOSE = "PUT /api/v1/projects/demo/account";

function workers(account: ProjectAccount) {
  return { body: workerList({ account }) };
}

function show(canWrite = true) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={queryClient}>
      <AccountPanel slug="demo" canWrite={canWrite} className="" />
    </QueryClientProvider>,
  );
}

function select(): HTMLSelectElement {
  return screen.getByLabelText<HTMLSelectElement>("Account");
}

describe("AccountPanel", () => {
  it("chooses an account and stays pending until the daemon answers", async () => {
    let chosen = "system-default";
    let answer: () => void = () => {};
    const answered = new Promise<void>((resolve) => {
      answer = resolve;
    });
    const calls = stubFetch({
      [WORKERS]: () =>
        workers({
          account: chosen,
          accounts: ["system-default", "work-a", "work-b"],
        }),
      [CHOOSE]: async () => {
        await answered;
        chosen = "work-b";
        return {
          body: {
            account: "work-b",
            accounts: ["system-default", "work-a", "work-b"],
          },
        };
      },
    });
    show();
    expect((await screen.findByLabelText("Account")).tagName).toBe("SELECT");
    expect(select().value).toBe("system-default");
    expect(screen.getByText(/continues in its own session/)).toBeTruthy();
    await userEvent.selectOptions(select(), "work-b");
    expect(await screen.findByText("Saving…")).toBeTruthy();
    expect(select().disabled).toBe(true);
    expect(select().value).toBe("work-b");
    answer();
    await waitFor(() => {
      expect(select().disabled).toBe(false);
    });
    expect(select().value).toBe("work-b");
    expect(calls.find((call) => call.method === "PUT")?.body).toEqual({
      account: "work-b",
    });
  });

  it("shows a refused choice with its code, keeps the project's account and reads the accounts again", async () => {
    let accounts = ["system-default", "work-a", "work-b"];
    stubFetch({
      [WORKERS]: () => workers({ account: "work-a", accounts }),
      [CHOOSE]: () => {
        accounts = ["system-default", "work-a"];
        return errorReply(404, "account_not_found");
      },
    });
    show();
    await screen.findByLabelText("Account");
    expect(select().value).toBe("work-a");
    await userEvent.selectOptions(select(), "work-b");
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("no such account");
    expect(alert.textContent).toContain("code: account_not_found");
    expect(select().value).toBe("work-a");
    await waitFor(() => {
      expect([...select().options].map((option) => option.value)).toEqual([
        "system-default",
        "work-a",
      ]);
    });
  });

  it("lets nobody but a signed-in person choose, and shows a failed read", async () => {
    stubFetch({
      [WORKERS]: workers({
        account: "system-default",
        accounts: ["system-default"],
      }),
    });
    show(false);
    await screen.findByLabelText("Account");
    expect(select().disabled).toBe(true);
    expect(screen.getByText(/fld account add <name>/)).toBeTruthy();
    cleanup();
    stubFetch({ [WORKERS]: errorReply(503, "project_unavailable") });
    show();
    expect(
      await screen.findByText("Could not load the project's account"),
    ).toBeTruthy();
  });
});
