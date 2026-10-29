// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  Outlet,
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import { act, cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { keys } from "../api/keys";
import { liveStatusStore } from "../live/status";
import { agent } from "../test/agents";
import { message, recipient, thread } from "../test/messages";
import { ThemeProvider } from "../theme/ThemeProvider";
import { PhoneMenu } from "./PhoneMenu";

afterEach(() => {
  cleanup();
  liveStatusStore.reset();
});

// An id such as m-1 looks like a margin utility to the token guard, so the test builds it.
const FIRST = `m-${String(1)}`;
const SECOND = `m-${String(2)}`;

async function renderMenu(pausedSince: number | null) {
  const client = new QueryClient({
    defaultOptions: { queries: { staleTime: Infinity, retry: false } },
  });
  client.setQueryData(keys.project("demo"), {
    overview: { paused_since: pausedSince },
  });
  client.setQueryData(keys.session(), { human: "tanim", authenticated: true });
  client.setQueryData(keys.needsYou("demo"), []);
  // One chat whose two questions wait: the tab counts questions, as the chat row does.
  const ask = (id: string) =>
    message(id, {
      sender: "web-lead",
      recipients: [recipient("human:tanim")],
      choices: [{ label: "Yes", description: null }],
    });
  client.setQueryData(keys.messages("demo"), {
    items: [
      thread(FIRST, {
        awaiting_human: true,
        participants: ["web-lead", "human:tanim"],
        messages: [ask(FIRST), ask(SECOND)],
      }),
    ],
    more: false,
  });
  client.setQueryData(keys.agentBar("demo"), [
    agent("web-lead"),
    agent("docs", { presence: "offline" }),
  ]);
  const root = createRootRoute({
    component: () => (
      <>
        <PhoneMenu slug="demo" />
        <Outlet />
      </>
    ),
  });
  const page = createRoute({
    getParentRoute: () => root,
    path: "/p/$slug/$tab",
    component: function Tab() {
      const { tab } = page.useParams();
      return <p>Tab {tab}</p>;
    },
  });
  const router = createRouter({
    routeTree: root.addChildren([page]),
    history: createMemoryHistory({ initialEntries: ["/p/demo/board"] }),
  });
  render(
    <ThemeProvider>
      <QueryClientProvider client={client}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </ThemeProvider>,
  );
  await screen.findByText("Tab board");
}

describe("PhoneMenu", () => {
  it("holds the tabs with their counts, the agents and the theme, and closes when a tab opens", async () => {
    liveStatusStore.set({ state: "live" });
    await renderMenu(null);
    await userEvent.click(screen.getByRole("button", { name: "Menu" }));
    const menu = screen.getByRole("dialog", { name: "Menu" });
    const tabs = within(menu).getByRole("navigation", { name: "Project tabs" });
    expect(
      within(tabs)
        .getByRole("link", { name: "Board" })
        .getAttribute("aria-current"),
    ).toBe("page");
    expect(
      within(tabs).getByRole("status", {
        name: "2 questions wait for your answer",
      }).textContent,
    ).toBe("2");
    expect(within(menu).getByText("1 working · 2 in all")).toBeTruthy();
    expect(within(menu).getByRole("group", { name: "Theme" })).toBeTruthy();
    expect(within(menu).getByRole("button", { name: "Sign out" })).toBeTruthy();
    await userEvent.click(within(tabs).getByRole("link", { name: "Agents" }));
    expect(await screen.findByText("Tab agents")).toBeTruthy();
    expect(screen.queryByRole("dialog", { name: "Menu" })).toBeNull();
  });

  it("names the paused agents and the reconnecting stream it holds", async () => {
    liveStatusStore.set({ state: "live" });
    await renderMenu(1_000);
    expect(
      screen.getByRole("button", { name: "Menu, Agents paused" }),
    ).toBeTruthy();
    act(() => {
      liveStatusStore.set({ state: "reconnecting" });
    });
    expect(
      screen.getByRole("button", { name: "Menu, Reconnecting" }),
    ).toBeTruthy();
  });
});
