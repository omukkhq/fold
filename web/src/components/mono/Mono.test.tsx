// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Mono } from "./Mono";

afterEach(cleanup);

describe("Mono", () => {
  it("renders an id in the mono face", () => {
    render(<Mono>T-12</Mono>);
    expect(screen.getByText("T-12").className).toContain("font-mono");
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("renders a link with href", () => {
    render(<Mono href="/tasks/12">T-12</Mono>);
    const link = screen.getByRole("link", { name: "T-12" });
    expect(link.getAttribute("href")).toBe("/tasks/12");
    expect(link.className).toContain("font-mono");
  });
});
