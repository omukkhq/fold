// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Label } from "./Label";

afterEach(cleanup);

describe("Label", () => {
  it("shows the name with the pair of its colour", () => {
    render(<Label color="teal">phase-1</Label>);
    const chip = screen.getByText("phase-1");
    expect(chip.className).toContain("bg-label-teal-soft");
    expect(chip.querySelector("span")?.className).toContain("bg-label-teal");
  });

  it("falls back to stone for an unknown colour", () => {
    render(<Label color="nope">x</Label>);
    expect(screen.getByText("x").className).toContain("bg-label-stone-soft");
  });
});
