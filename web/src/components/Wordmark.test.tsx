// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Wordmark } from "./Wordmark";

afterEach(cleanup);

describe("Wordmark", () => {
  it("is named fold and colours the dot with the primary token", () => {
    const { container } = render(<Wordmark />);
    expect(screen.getByLabelText("fold").textContent).toBe("fold.");
    const dot = container.querySelector("[aria-hidden]");
    expect(dot?.textContent).toBe(".");
    expect(dot?.classList.contains("text-accent")).toBe(true);
  });
});
