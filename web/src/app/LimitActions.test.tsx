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

/** The left part of the control, which opens the list. */
function trigger(): HTMLButtonElement {
  const found = screen
    .getAllByRole<HTMLButtonElement>("button")
    .find((button) => button.getAttribute("aria-haspopup") === "menu");
  if (found === undefined) {
    throw new Error("no Switch account trigger");
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

async function choose(name: string) {
  await openList();
  await userEvent.click(screen.getByRole("menuitem", { name }));
}

describe("LimitActions", () => {
  it("offers one target in the list under Switch account, with the switch button off until a choice", async () => {
    stubFetch({
      [SESSION]: { body: { human: "tanim", authenticated: true } },
    });
    show(["work-b"]);
    expect(await openList()).toEqual(["work-b"]);
    expect(screen.getByRole("menu").className).toContain(
      "w-(--radix-dropdown-menu-trigger-width)",
    );
    await userEvent.keyboard("{Escape}");
    expect(
      screen.getByRole<HTMLButtonElement>("button", {
        name: "Choose an account first",
      }).disabled,
    ).toBe(true);
  });

  it("only picks an account on a choice, and switches to it on the switch button, pending until the daemon answers", async () => {
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
    expect(trigger().textContent).toBe("work-b");
    expect(calls.some((call) => call.method === "PUT")).toBe(false);
    const send = screen.getByRole<HTMLButtonElement>("button", {
      name: "Switch to work-b",
    });
    expect(send.disabled).toBe(false);
    await userEvent.click(send);
    expect(
      await screen.findByRole("button", { name: "Switching to work-b…" }),
    ).toBeTruthy();
    expect(trigger().disabled).toBe(true);
    expect(
      screen.getByRole<HTMLButtonElement>("button", {
        name: "Switch to work-b",
      }).disabled,
    ).toBe(true);
    answer();
    await screen.findByRole("button", { name: "Switch account" });
    expect(
      screen.getByRole<HTMLButtonElement>("button", {
        name: "Choose an account first",
      }).disabled,
    ).toBe(true);
    expect(calls.find((call) => call.method === "PUT")?.body).toEqual({
      account: "work-b",
    });
  });

  it("shows a refused switch with its code, resets the choice and reads the item and the accounts again", async () => {
    stubFetch({
      [SESSION]: { body: { human: "tanim", authenticated: true } },
      [CHOOSE]: errorReply(404, "account_not_found"),
    });
    const { invalidated } = show();
    await choose("personal");
    await userEvent.click(
      screen.getByRole("button", { name: "Switch to personal" }),
    );
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("no such account");
    expect(alert.textContent).toContain("account_not_found");
    const keysRead = invalidated.mock.calls.map((call) => call[0]?.queryKey);
    expect(keysRead).toContainEqual(keys.needsYou("demo"));
    expect(keysRead).toContainEqual(keys.workers("demo"));
    expect(trigger().textContent).toBe("Switch account");
    expect(
      screen.getByRole<HTMLButtonElement>("button", {
        name: "Choose an account first",
      }).disabled,
    ).toBe(true);
  });

  it("lists three or more targets in the same list, Claude Code's own login by its name", async () => {
    stubFetch({
      [SESSION]: { body: { human: "tanim", authenticated: true } },
    });
    show(["system-default", "work-b", "work-c", "personal"]);
    expect(await openList()).toEqual([
      "system-default (Claude Code's own login)",
      "work-b",
      "work-c",
      "personal",
    ]);
    await userEvent.click(
      screen.getByRole("menuitem", {
        name: "system-default (Claude Code's own login)",
      }),
    );
    expect(trigger().textContent).toBe(
      "system-default (Claude Code's own login)",
    );
    expect(
      screen.getByRole("button", { name: "Switch to system-default" }),
    ).toBeTruthy();
  });

  it("offers no switch without a target or to a browser that names no person", async () => {
    stubFetch({
      [SESSION]: { body: { human: "tanim", authenticated: true } },
    });
    const { client } = show([]);
    await waitFor(() => {
      expect(client.getQueryData(sessionQuery().queryKey)).toBeDefined();
    });
    expect(screen.queryByRole("button", { name: /Switch|Choose/ })).toBeNull();
    cleanup();
    stubFetch({ [SESSION]: { body: { human: null, authenticated: true } } });
    const other = show().client;
    await waitFor(() => {
      expect(other.getQueryData(sessionQuery().queryKey)).toBeDefined();
    });
    expect(screen.queryByRole("button", { name: /Switch|Choose/ })).toBeNull();
  });
});
