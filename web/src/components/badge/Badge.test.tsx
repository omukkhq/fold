// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { BADGE_TONES } from "./badges";
import { Badge } from "./Badge";

afterEach(cleanup);

describe("Badge", () => {
  it.each(BADGE_TONES)("shows text for %s, not colour alone", (tone) => {
    const { container } = render(<Badge tone={tone} />);
    expect(container.textContent.length).toBeGreaterThan(0);
    expect(container.querySelector("svg")?.getAttribute("aria-hidden")).toBe(
      "true",
    );
  });

  it("shows the caller's text", () => {
    render(<Badge tone="stalled">Stalled for 4m</Badge>);
    expect(screen.getByText("Stalled for 4m").getAttribute("data-tone")).toBe(
      "stalled",
    );
  });
});
