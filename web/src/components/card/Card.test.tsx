// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Card } from "./Card";

afterEach(cleanup);

describe("Card", () => {
  it("renders header, body and footer slots", () => {
    render(
      <Card header={<h3>Head</h3>} footer={<span>Foot</span>}>
        Body
      </Card>,
    );
    expect(screen.getByRole("heading", { name: "Head" })).toBeTruthy();
    expect(screen.getByText("Body")).toBeTruthy();
    expect(screen.getByText("Foot")).toBeTruthy();
  });

  it("omits the footer row without a footer", () => {
    const { container } = render(<Card>Body</Card>);
    expect(container.querySelector(".border-t")).toBeNull();
  });

  it("marks the selected card", () => {
    render(
      <Card selected data-testid="card">
        Body
      </Card>,
    );
    const card = screen.getByTestId("card");
    expect(card.getAttribute("aria-current")).toBe("true");
    expect(card.hasAttribute("data-selected")).toBe(true);
  });

  it("is not marked by default and can take focus from the caller", () => {
    render(
      <Card tabIndex={0} data-testid="card">
        Body
      </Card>,
    );
    const card = screen.getByTestId("card");
    expect(card.getAttribute("aria-current")).toBeNull();
    card.focus();
    expect(document.activeElement).toBe(card);
  });
});
