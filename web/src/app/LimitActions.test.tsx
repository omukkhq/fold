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

function dropdown(): HTMLSelectElement {
  return screen.getByLabelText<HTMLSelectElement>("Switch account");
}

/** The words of the dropdown's options, its first one the placeholder. */
function options(): (string | null)[] {
  return [...dropdown().options].map((option) => option.textContent);
}

describe("LimitActions", () => {
  it("offers one target in the one Switch account dropdown, with nothing chosen ahead and no menu", async () => {
    stubFetch({
      [SESSION]: { body: { human: "tanim", authenticated: true } },
    });
    show(["work-b"]);
    await screen.findByLabelText("Switch account");
    expect(options()).toEqual(["Switch account", "work-b"]);
    expect(dropdown().value).toBe("");
    expect(screen.queryByRole("button", { name: /Switch/ })).toBeNull();
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("switches at once to the chosen account of two and stays pending until the daemon answers", async () => {
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
    await screen.findByLabelText("Switch account");
    expect(options()).toEqual(["Switch account", "work-b", "personal"]);
    await userEvent.selectOptions(dropdown(), "work-b");
    await waitFor(() => {
      expect(dropdown().disabled).toBe(true);
    });
    expect(options()[0]).toBe("Switching to work-b…");
    expect(dropdown().value).toBe("");
    answer();
    await waitFor(() => {
      expect(dropdown().disabled).toBe(false);
    });
    expect(options()[0]).toBe("Switch account");
    expect(calls.find((call) => call.method === "PUT")?.body).toEqual({
      account: "work-b",
    });
  });

  it("shows a refused switch with its code, chooses nothing and reads the item and the accounts again", async () => {
    stubFetch({
      [SESSION]: { body: { human: "tanim", authenticated: true } },
      [CHOOSE]: errorReply(404, "account_not_found"),
    });
    const { invalidated } = show();
    await screen.findByLabelText("Switch account");
    await userEvent.selectOptions(dropdown(), "personal");
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("no such account");
    expect(alert.textContent).toContain("account_not_found");
    const keysRead = invalidated.mock.calls.map((call) => call[0]?.queryKey);
    expect(keysRead).toContainEqual(keys.needsYou("demo"));
    expect(keysRead).toContainEqual(keys.workers("demo"));
    expect(dropdown().disabled).toBe(false);
    expect(dropdown().value).toBe("");
  });

  it("offers three or more targets in the same dropdown, Claude Code's own login by its name", async () => {
    stubFetch({
      [SESSION]: { body: { human: "tanim", authenticated: true } },
    });
    show(["system-default", "work-b", "work-c", "personal"]);
    await screen.findByLabelText("Switch account");
    expect(options()).toEqual([
      "Switch account",
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
    expect(screen.queryByLabelText("Switch account")).toBeNull();
    cleanup();
    stubFetch({ [SESSION]: { body: { human: null, authenticated: true } } });
    const other = show().client;
    await waitFor(() => {
      expect(other.getQueryData(sessionQuery().queryKey)).toBeDefined();
    });
    expect(screen.queryByLabelText("Switch account")).toBeNull();
  });
});
