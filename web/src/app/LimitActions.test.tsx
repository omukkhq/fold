// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { keys } from "../api/keys";
import { sessionQuery } from "../api/queries";
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
  const invalidated = vi.spyOn(client, "invalidateQueries");
  render(
    <QueryClientProvider client={client}>
      <LimitActions slug="demo" accounts={accounts} />
    </QueryClientProvider>,
  );
  return { client, invalidated };
}

/** Opens the one Switch account menu and gives the words of its items. */
async function openMenu(): Promise<string[]> {
  await userEvent.click(
    await screen.findByRole("button", { name: "Switch account" }),
  );
  return screen.getAllByRole("menuitem").map((item) => item.textContent);
}

describe("LimitActions", () => {
  it("offers one target in the one Switch account menu, never a button of its own, next to Pause", async () => {
    stubFetch({
      [SESSION]: { body: { human: "tanim", authenticated: true } },
    });
    show(["work-b"]);
    expect(await openMenu()).toEqual(["work-b"]);
    expect(screen.queryByRole("button", { name: /Switch to/ })).toBeNull();
    expect(screen.getByText(/each waiting turn runs again on it/)).toBeTruthy();
  });

  it("switches to the chosen account of two and stays pending until the daemon answers", async () => {
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
    expect(await openMenu()).toEqual(["work-b", "personal"]);
    await userEvent.click(screen.getByRole("menuitem", { name: "work-b" }));
    expect(
      await screen.findByRole("button", { name: "Switching to work-b…" }),
    ).toBeTruthy();
    answer();
    await screen.findByRole("button", { name: "Switch account" });
    expect(calls.find((call) => call.method === "PUT")?.body).toEqual({
      account: "work-b",
    });
  });

  it("shows a refused switch with its code and reads the item and the accounts again", async () => {
    stubFetch({
      [SESSION]: { body: { human: "tanim", authenticated: true } },
      [CHOOSE]: errorReply(404, "account_not_found"),
    });
    const { invalidated } = show();
    await openMenu();
    await userEvent.click(screen.getByRole("menuitem", { name: "personal" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("no such account");
    expect(alert.textContent).toContain("code: account_not_found");
    const keysRead = invalidated.mock.calls.map((call) => call[0]?.queryKey);
    expect(keysRead).toContainEqual(keys.needsYou("demo"));
    expect(keysRead).toContainEqual(keys.workers("demo"));
    await waitFor(() => {
      expect(
        screen.getByRole<HTMLButtonElement>("button", {
          name: "Switch account",
        }).disabled,
      ).toBe(false);
    });
  });

  it("offers three or more targets in the same menu, Claude Code's own login by its name", async () => {
    stubFetch({
      [SESSION]: { body: { human: "tanim", authenticated: true } },
    });
    show(["system-default", "work-b", "work-c", "personal"]);
    expect(await openMenu()).toEqual([
      "system-default (Claude Code's own login)",
      "work-b",
      "work-c",
      "personal",
    ]);
  });

  it("offers no switch without a target or to a browser that names no person", async () => {
    stubFetch({
      [SESSION]: { body: { human: "tanim", authenticated: true } },
    });
    const { client } = show([]);
    await waitFor(() => {
      expect(client.getQueryData(sessionQuery().queryKey)).toBeDefined();
    });
    expect(screen.queryByRole("button", { name: "Switch account" })).toBeNull();
    cleanup();
    stubFetch({ [SESSION]: { body: { human: null, authenticated: true } } });
    show();
    await waitFor(() => {
      expect(
        screen.queryByRole("button", { name: "Switch account" }),
      ).toBeNull();
    });
  });
});
