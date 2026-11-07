// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "@/test/stubFetch";
import { workerProcess } from "@/test/workers";
import { RestartStalled } from "./RestartStalled";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const URL = "POST /api/v1/projects/demo/workers/web-worker/restart";

function show() {
  const client = new QueryClient({
    defaultOptions: { mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <RestartStalled slug="demo" name="web-worker" />
    </QueryClientProvider>,
  );
}

describe("RestartStalled", () => {
  it("starts the worker again in a new session with one press and stays pending until the daemon answers", async () => {
    let answer: () => void = () => undefined;
    const answered = new Promise<void>((resolve) => {
      answer = resolve;
    });
    const calls = stubFetch({
      [URL]: async () => {
        await answered;
        return { body: workerProcess("web-worker", { state: "starting" }) };
      },
    });
    show();
    fireEvent.click(
      screen.getByRole("button", {
        name: "Restart web-worker in a new session",
      }),
    );
    expect(await screen.findByText("Restarting…")).toBeTruthy();
    answer();
    expect(await screen.findByText("Restart worker")).toBeTruthy();
    expect(calls[0]?.body).toEqual({});
  });

  it("says why the daemon refused the restart", async () => {
    stubFetch({ [URL]: errorReply(409, "project_paused") });
    show();
    fireEvent.click(screen.getByRole("button"));
    expect((await screen.findByRole("alert")).textContent).toContain(
      "The agents of this project are paused",
    );
  });
});
