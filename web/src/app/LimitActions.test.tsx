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

/** The control's opener, which opens the list. */
function opener(): HTMLButtonElement {
  const found = screen
    .getAllByRole<HTMLButtonElement>("button")
    .find((button) => button.getAttribute("aria-haspopup") === "menu");
  if (found === undefined) {
    throw new Error("no Switch account opener");
  }
  return found;
}

/** Opens the list and gives the words of its entries. */
async function openList(): Promise<string[]> {
  await userEvent.click(
    await screen.findByRole("button", { name: "Switch account" }),
  );
  return (await screen.findAllByRole("menuitem")).map(
    (item) => item.textContent,
  );
}

describe("LimitActions", () => {
  it("offers one target in the list under Switch account, by its name only, and no separate switch button", async () => {
    stubFetch({
      [SESSION]: { body: { human: "tanim", authenticated: true } },
    });
    show(["default"]);
    await screen.findByRole("button", { name: "Switch account" });
    expect(screen.getAllByRole("button")).toHaveLength(1);
    expect(opener().className).toContain("w-switch-account");
    expect(await openList()).toEqual(["default"]);
    expect(screen.getByRole("menu").className).toContain(
      "min-w-(--radix-dropdown-menu-trigger-width)",
    );
  });

  it("switches at once to the chosen account of two and stays pending on the opener until the daemon answers", async () => {
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
    expect(await openList()).toEqual(["work-b", "personal"]);
    await userEvent.click(screen.getByRole("menuitem", { name: "work-b" }));
    expect(
      await screen.findByRole("button", { name: "Switching to work-b…" }),
    ).toBeTruthy();
    expect(opener().disabled).toBe(true);
    await waitFor(() => {
      expect(calls.find((call) => call.method === "PUT")?.body).toEqual({
        account: "work-b",
      });
    });
    answer();
    await screen.findByRole("button", { name: "Switch account" });
    expect(opener().disabled).toBe(false);
  });

  it("shows a refused switch with its code and reads the item and the accounts again", async () => {
    stubFetch({
      [SESSION]: { body: { human: "tanim", authenticated: true } },
      [CHOOSE]: errorReply(404, "account_not_found"),
    });
    const { invalidated } = show();
    await openList();
    await userEvent.click(screen.getByRole("menuitem", { name: "personal" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("no such account");
    expect(alert.textContent).toContain("account_not_found");
    const keysRead = invalidated.mock.calls.map((call) => call[0]?.queryKey);
    expect(keysRead).toContainEqual(keys.needsYou("demo"));
    expect(keysRead).toContainEqual(keys.workers("demo"));
    expect(opener().textContent).toBe("Switch account");
    expect(opener().disabled).toBe(false);
  });

  it("lists three or more targets in the same list, default by its name", async () => {
    stubFetch({
      [SESSION]: { body: { human: "tanim", authenticated: true } },
    });
    show(["default", "work-b", "work-c", "personal"]);
    expect(await openList()).toEqual([
      "default",
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
    expect(screen.queryByRole("button", { name: /Switch/ })).toBeNull();
    cleanup();
    stubFetch({ [SESSION]: { body: { human: null, authenticated: true } } });
    const other = show().client;
    await waitFor(() => {
      expect(other.getQueryData(sessionQuery().queryKey)).toBeDefined();
    });
    expect(screen.queryByRole("button", { name: /Switch/ })).toBeNull();
  });
});
