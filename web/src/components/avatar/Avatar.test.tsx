// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Avatar, PresenceDot } from "./Avatar";

afterEach(cleanup);

describe("Avatar", () => {
  it("shows the initials and is named after the agent", () => {
    render(<Avatar name="frontend/ui" />);
    expect(screen.getByRole("img", { name: "frontend/ui" }).textContent).toBe(
      "FU",
    );
  });

  it("shows one letter for a human", () => {
    render(<Avatar name="human:tanim" />);
    expect(screen.getByRole("img", { name: "human:tanim" }).textContent).toBe(
      "T",
    );
  });
});

describe("PresenceDot", () => {
  it.each([
    ["active", "Active"],
    ["idle", "Idle"],
    ["offline", "Offline"],
  ] as const)("names the %s state in words", (state, name) => {
    render(<PresenceDot state={state} />);
    expect(screen.getByRole("img", { name })).toBeTruthy();
  });
});
