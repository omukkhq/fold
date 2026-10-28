// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { IdText } from "./IdText";

afterEach(cleanup);

describe("IdText", () => {
  it("draws an id in the body face with tabular numbers, never broken", () => {
    render(<IdText className="text-13">T-12</IdText>);
    const id = screen.getByText("T-12");
    expect(id.className).toContain("font-body");
    expect(id.className).toContain("tabular-nums");
    expect(id.className).toContain("whitespace-nowrap");
    expect(id.className).not.toContain("font-mono");
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("renders a link with href", () => {
    render(<IdText href="/p/demo/proposals/P-3">P-3</IdText>);
    const link = screen.getByRole("link", { name: "P-3" });
    expect(link.getAttribute("href")).toBe("/p/demo/proposals/P-3");
    expect(link.className).toContain("text-accent");
    expect(link.className).toContain("tabular-nums");
  });
});
