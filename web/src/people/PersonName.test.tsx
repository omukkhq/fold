// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ActorText } from "./ActorText";
import { HumanReviewBadge } from "./HumanReviewBadge";
import { PersonList, PersonName } from "./PersonName";
import { actorSegment, seg } from "./segments";

afterEach(cleanup);

describe("PersonName", () => {
  it("draws a human as @name in the link-text colour, with no avatar and no prefix", () => {
    const { container } = render(<PersonName name="human:tanim" />);
    const name = screen.getByText("@tanim");
    expect(name.getAttribute("data-person")).toBe("human");
    expect(name.className).toContain("text-primary-text");
    expect(name.className).toContain("font-medium");
    expect(container.querySelector('[role="img"]')).toBeNull();
  });

  it("draws an agent as @name in the secondary colour", () => {
    render(<PersonName name="frontend/ui" />);
    const name = screen.getByText("@frontend/ui");
    expect(name.getAttribute("data-person")).toBe("agent");
    expect(name.className).toContain("text-secondary-text");
    expect(name.className).not.toContain("text-primary-text");
  });

  it("takes the caller's size but keeps Barlow, the weight and its colour", () => {
    render(
      <PersonName
        name="backend/db"
        className="font-mono text-code font-semibold text-text-muted"
      />,
    );
    const name = screen.getByText("@backend/db");
    expect(name.className).toContain("text-code");
    expect(name.className).toContain("font-ui");
    expect(name.className).toContain("font-medium");
    expect(name.className).toContain("text-secondary-text");
    expect(name.className).not.toContain("font-mono");
    expect(name.className).not.toContain("font-semibold");
    expect(name.className).not.toContain("text-text-muted");
  });
});

describe("PersonList", () => {
  it("joins the names with commas, each in its own colour and the given size", () => {
    const { container } = render(
      <PersonList names={["frontend", "human:ana"]} nameClassName="text-bar" />,
    );
    expect(container.textContent).toBe("@frontend, @ana");
    const names = container.querySelectorAll("[data-person]");
    expect(names).toHaveLength(2);
    expect(names[1]?.className).toContain("text-bar");
  });
});

describe("ActorText", () => {
  it("draws the actors of a line as @names in the given size", () => {
    const { container } = render(
      <ActorText
        segments={seg`Claimed by ${actorSegment("frontend/ui")} · 13:40`}
        personClassName="text-bar"
      />,
    );
    expect(container.textContent).toBe("Claimed by @frontend/ui · 13:40");
    expect(screen.getByText("@frontend/ui").className).toContain("text-bar");
  });
});

describe("HumanReviewBadge", () => {
  it("names the reviewer as @name, or no one", () => {
    const { container, rerender } = render(
      <HumanReviewBadge reviewer="tanim" />,
    );
    expect(container.textContent).toContain("Human review");
    expect(container.querySelector('[data-person="human"]')?.textContent).toBe(
      "@tanim",
    );
    rerender(<HumanReviewBadge reviewer={null} />);
    expect(container.querySelector("[data-person]")).toBeNull();
  });
});
