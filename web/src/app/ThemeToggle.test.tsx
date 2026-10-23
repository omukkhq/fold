// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ThemeProvider } from "../theme/ThemeProvider";
import { ThemeToggle } from "./ThemeToggle";

afterEach(() => {
  cleanup();
});

describe("ThemeToggle", () => {
  it("is an icon button in the row and a labelled row in the More popover", () => {
    const { rerender } = render(
      <ThemeProvider>
        <ThemeToggle />
      </ThemeProvider>,
    );
    expect(
      screen.getByRole("button", { name: "Toggle theme" }).textContent,
    ).toBe("");

    rerender(
      <ThemeProvider>
        <ThemeToggle variant="row" />
      </ThemeProvider>,
    );
    expect(
      screen.getByRole("button", { name: "Toggle theme" }).textContent,
    ).toBe("Toggle theme");
  });
});
