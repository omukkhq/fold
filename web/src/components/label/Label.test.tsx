// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Label } from "./Label";

afterEach(cleanup);

describe("Label", () => {
  it("shows the name after a dot of its colour, with no chip around it", () => {
    render(<Label color="teal">phase-1</Label>);
    const label = screen.getByText("phase-1");
    expect(label.className).not.toContain("inset-ring");
    expect(label.className).not.toContain("bg-");
    expect(label.querySelector("span")?.className).toContain("bg-label-teal");
  });

  it("falls back to stone for an unknown colour", () => {
    render(<Label color="nope">x</Label>);
    expect(screen.getByText("x").querySelector("span")?.className).toContain(
      "bg-label-stone",
    );
  });
});
