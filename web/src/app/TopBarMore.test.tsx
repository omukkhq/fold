// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { keys } from "../api/keys";
import { liveStatusStore } from "../live/status";
import { TopBarMore } from "./TopBarMore";

afterEach(() => {
  cleanup();
  liveStatusStore.reset();
});

function renderMore(pausedSince: number | null) {
  const client = new QueryClient({
    defaultOptions: { queries: { staleTime: Infinity, retry: false } },
  });
  client.setQueryData(keys.project("demo"), {
    overview: { paused_since: pausedSince },
  });
  render(
    <QueryClientProvider client={client}>
      <TopBarMore slug="demo">
        {(close) => (
          <button type="button" onClick={close}>
            Open the board
          </button>
        )}
      </TopBarMore>
    </QueryClientProvider>,
  );
}

describe("TopBarMore", () => {
  it("opens its items and closes when an item asks", async () => {
    liveStatusStore.set({ state: "live" });
    renderMore(null);
    await userEvent.click(screen.getByRole("button", { name: "More" }));
    expect(screen.getByRole("dialog", { name: "More" })).toBeTruthy();
    await userEvent.click(
      screen.getByRole("button", { name: "Open the board" }),
    );
    expect(screen.queryByRole("dialog", { name: "More" })).toBeNull();
  });

  it("names the paused agents and the reconnecting stream it holds", () => {
    liveStatusStore.set({ state: "live" });
    renderMore(1_000);
    expect(
      screen.getByRole("button", { name: "More, Agents paused" }),
    ).toBeTruthy();
    act(() => {
      liveStatusStore.set({ state: "reconnecting" });
    });
    expect(
      screen.getByRole("button", { name: "More, Reconnecting" }),
    ).toBeTruthy();
  });
});
