// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import { PauseControl } from "./PauseControl";
import { PausedBanner } from "./PausedBanner";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const AT = new Date(2026, 9, 17, 23, 40).getTime();

function project(pausedSince: number | null) {
  return {
    slug: "demo",
    name: "demo",
    available: true,
    overview: {
      counts: { backlog: 0, todo: 0, in_progress: 0, review: 0, done: 0 },
      agents: [],
      head: null,
      last_change_at: null,
      attention: [],
      paused_since: pausedSince,
      paused_by: pausedSince === null ? null : "human:tanim",
    },
  };
}

function renderBar() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <PauseControl slug="demo" />
      <PausedBanner slug="demo" />
    </QueryClientProvider>,
  );
}

describe("PauseControl", () => {
  it("pauses, waits for the daemon's state, then offers the resume and shows the banner", async () => {
    let paused: number | null = null;
    let release = () => {};
    const answered = new Promise<void>((resolve) => {
      release = resolve;
    });
    stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects/demo": () => ({ body: project(paused) }),
      "POST /api/v1/projects/demo/pause": async () => {
        await answered;
        paused = AT;
        return { body: { paused_since: AT, paused_by: "human:tanim" } };
      },
    });
    renderBar();
    const button = await screen.findByRole("button", { name: "Pause agents" });
    expect(screen.queryByRole("status")).toBeNull();
    await userEvent.click(button);
    const pending = await screen.findByRole("button", { name: "Pausing…" });
    expect(pending.hasAttribute("disabled")).toBe(true);
    release();
    await screen.findByRole("button", { name: "Resume agents" });
    expect(screen.getByRole("status").textContent).toContain(
      "Agents paused since 23:40 by @tanim",
    );
    const name = screen.getByText("@tanim");
    expect(name.getAttribute("data-person")).toBe("human");
    expect(name.className).toContain("text-primary-text");
  });

  it("shows the refusal of a resume", async () => {
    stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects/demo": { body: project(AT) },
      "POST /api/v1/projects/demo/resume": errorReply(
        503,
        "project_unavailable",
      ),
    });
    renderBar();
    await userEvent.click(
      await screen.findByRole("button", { name: "Resume agents" }),
    );
    expect((await screen.findByRole("alert")).textContent).toBe(
      "The project is not available now.",
    );
  });
});
