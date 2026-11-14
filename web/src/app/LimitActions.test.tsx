// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import { LimitActions } from "./LimitActions";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const SESSION = "GET /api/v1/session";
const CHOOSE = "PUT /api/v1/projects/demo/account";

function show(accounts: readonly string[] = ["work-b", "personal"]) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <LimitActions slug="demo" accounts={accounts} />
    </QueryClientProvider>,
  );
}

describe("LimitActions", () => {
  it("switches to the chosen account and stays pending until the daemon answers", async () => {
    let answer: () => void = () => {};
    const answered = new Promise<void>((resolve) => {
      answer = resolve;
    });
    const calls = stubFetch({
      [SESSION]: { body: { human: "tanim", authenticated: true } },
      [CHOOSE]: async () => {
        await answered;
        return {
          body: { account: "work-b", accounts: ["personal", "work-b"] },
        };
      },
    });
    show();
    await userEvent.click(
      await screen.findByRole("button", { name: "Switch to work-b" }),
    );
    expect(
      await screen.findByRole("button", { name: "Switching to work-b…" }),
    ).toBeTruthy();
    expect(
      screen.getByRole<HTMLButtonElement>("button", {
        name: "Switch to personal",
      }).disabled,
    ).toBe(true);
    answer();
    await screen.findByRole("button", { name: "Switch to work-b" });
    expect(calls.find((call) => call.method === "PUT")?.body).toEqual({
      account: "work-b",
    });
  });

  it("shows a refused switch with its code", async () => {
    stubFetch({
      [SESSION]: { body: { human: "tanim", authenticated: true } },
      [CHOOSE]: errorReply(404, "account_not_found"),
    });
    show();
    await userEvent.click(
      await screen.findByRole("button", { name: "Switch to personal" }),
    );
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("no such account");
    expect(alert.textContent).toContain("code: account_not_found");
    await waitFor(() => {
      expect(
        screen.getByRole<HTMLButtonElement>("button", {
          name: "Switch to personal",
        }).disabled,
      ).toBe(false);
    });
  });

  it("offers three or more targets in one menu with its hint", async () => {
    let answer: () => void = () => {};
    const answered = new Promise<void>((resolve) => {
      answer = resolve;
    });
    const calls = stubFetch({
      [SESSION]: { body: { human: "tanim", authenticated: true } },
      [CHOOSE]: async () => {
        await answered;
        return {
          body: {
            account: "work-c",
            accounts: ["system-default", "work-b", "work-c"],
          },
        };
      },
    });
    show(["system-default", "work-b", "work-c"]);
    await userEvent.click(
      await screen.findByRole("button", { name: "Switch account" }),
    );
    expect(screen.queryByRole("button", { name: /Switch to/ })).toBeNull();
    expect(screen.getByText(/each waiting turn runs again on it/)).toBeTruthy();
    const items = screen.getAllByRole("menuitem");
    expect(items.map((item) => item.textContent)).toEqual([
      "system-default (Claude Code's own login)",
      "work-b",
      "work-c",
    ]);
    await userEvent.click(screen.getByRole("menuitem", { name: "work-c" }));
    expect(
      await screen.findByRole("button", { name: "Switching to work-c…" }),
    ).toBeTruthy();
    answer();
    await screen.findByRole("button", { name: "Switch account" });
    expect(calls.find((call) => call.method === "PUT")?.body).toEqual({
      account: "work-c",
    });
  });

  it("offers no switch to a browser that names no person", async () => {
    stubFetch({ [SESSION]: { body: { human: null, authenticated: true } } });
    show();
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: /Switch to/ })).toBeNull();
    });
  });
});
