// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { PersonList, PersonName } from "./PersonName";

afterEach(cleanup);

describe("PersonName", () => {
  it("shows a human in the link-text colour, with no avatar and no prefix", () => {
    render(<PersonName name="human:tanim" />);
    const name = screen.getByText("tanim");
    expect(name.className).toContain("text-primary-text");
    expect(name.getAttribute("data-person")).toBe("human");
    expect(screen.queryByRole("img")).toBeNull();
  });

  it("keeps Barlow and the human colour over the caller's font and colour", () => {
    render(
      <PersonName
        name="human:tanim"
        className="font-mono text-code text-text-muted"
      />,
    );
    const name = screen.getByText("tanim");
    expect(name.className).toContain("font-ui");
    expect(name.className).toContain("text-primary-text");
    expect(name.className).toContain("text-code");
    expect(name.className).not.toContain("font-mono");
    expect(name.className).not.toContain("text-text-muted");
  });

  it("draws an agent without its chip where the screen asks for none", () => {
    render(<PersonName name="backend/db" chip={false} className="font-mono" />);
    expect(screen.queryByRole("img")).toBeNull();
    expect(screen.getByText("backend/db").className).toBe("font-mono");
  });

  it("keeps the worker chip for an agent", () => {
    render(<PersonName name="frontend/ui" />);
    expect(screen.getByRole("img", { name: "frontend/ui" })).toBeTruthy();
    expect(screen.getByText("frontend/ui", { selector: "span" })).toBeTruthy();
  });
});

describe("PersonList", () => {
  it("separates the names and colours the humans", () => {
    const { container } = render(
      <PersonList names={["frontend", "human:ana"]} />,
    );
    expect(container.textContent).toBe("frontend, ana");
    expect(screen.getByText("ana").className).toContain("text-primary-text");
  });
});
