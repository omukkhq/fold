// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Inbox } from "lucide-react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { EmptyPanel } from "./EmptyPanel";
import { ErrorBanner } from "./ErrorBanner";
import { ReconnectingBanner } from "./ReconnectingBanner";
import { Skeleton } from "./Skeleton";

afterEach(cleanup);

describe("Skeleton", () => {
  it("announces loading and is busy", () => {
    render(<Skeleton />);
    const skeleton = screen.getByRole("status", { name: "Loading" });
    expect(skeleton.getAttribute("aria-busy")).toBe("true");
    expect(skeleton.children).toHaveLength(3);
  });

  it("stops the shimmer under reduced motion", () => {
    render(<Skeleton />);
    for (const bar of screen.getByRole("status").children) {
      expect(bar.className).toContain("motion-reduce:animate-none");
    }
  });

  it("sizes the bars 70, fill and 120 from tokens", () => {
    render(<Skeleton />);
    const bars = [...screen.getByRole("status").children];
    expect(bars.map((bar) => bar.className)).toEqual([
      expect.stringContaining("w-skeleton-s"),
      expect.stringContaining("w-full"),
      expect.stringContaining("w-skeleton-l"),
    ]);
  });
});

describe("EmptyPanel", () => {
  it("shows the title, the next step and the action", () => {
    render(
      <EmptyPanel
        icon={Inbox}
        title="No tasks yet"
        action={<button type="button">New task</button>}
      >
        Give an orchestrator a goal.
      </EmptyPanel>,
    );
    expect(screen.getByRole("heading", { name: "No tasks yet" })).toBeTruthy();
    expect(screen.getByText("Give an orchestrator a goal.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "New task" })).toBeTruthy();
  });

  it("works without an action", () => {
    render(
      <EmptyPanel icon={Inbox} title="Nothing here">
        Come back later.
      </EmptyPanel>,
    );
    expect(screen.queryByRole("button")).toBeNull();
  });
});

describe("ErrorBanner", () => {
  it("is an alert with the title and message", () => {
    render(
      <ErrorBanner
        title="Could not load"
        message="The daemon did not answer."
      />,
    );
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("Could not load");
    expect(alert.textContent).toContain("The daemon did not answer.");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("calls retry from a keyboard reachable button", async () => {
    const onRetry = vi.fn();
    render(<ErrorBanner title="t" message="m" onRetry={onRetry} />);
    await userEvent.tab();
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Retry" }),
    );
    await userEvent.keyboard("{Enter}");
    expect(onRetry).toHaveBeenCalledTimes(1);
  });
});

describe("ReconnectingBanner", () => {
  it("says updates paused, gives the time and says missed events replay", () => {
    const now = new Date(2026, 9, 15, 14, 20, 0).getTime();
    const asOf = new Date(2026, 9, 15, 14, 12, 31).getTime();
    render(<ReconnectingBanner now={now} asOf={asOf} />);
    const text = screen.getByRole("status").textContent;
    expect(text).toContain("Live updates paused");
    expect(text).toContain("14:12:31");
    expect(text).toContain("missed events replay");
  });

  it("offers retry when given a handler", async () => {
    const onRetry = vi.fn();
    render(<ReconnectingBanner now={2} asOf={1} onRetry={onRetry} />);
    await userEvent.click(screen.getByRole("button", { name: "Retry now" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });
});
