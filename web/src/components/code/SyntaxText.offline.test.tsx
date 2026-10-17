// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { highlightSegments } from "@/syntax/highlight";
import { SyntaxText } from "./SyntaxText";

vi.mock("@/syntax/shiki", () => {
  throw new Error("offline");
});

afterEach(cleanup);

describe("SyntaxText without Shiki", () => {
  it("keeps the code plain and shows no error when Shiki's core fails to load", async () => {
    const error = vi.spyOn(console, "error");
    render(
      <pre data-testid="code">
        <SyntaxText code="let x = 1;" language="typescript" />
      </pre>,
    );
    expect(await highlightSegments("typescript", [["let x = 1;"]])).toBeNull();
    await new Promise((resolve) => setTimeout(resolve, 0));
    const pre = screen.getByTestId("code");
    expect(pre.textContent).toBe("let x = 1;");
    expect(pre.querySelector("span")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(error).not.toHaveBeenCalled();
  });
});
