// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ClampText } from "./ClampText";

function textHeight(full: number, shown: number) {
  vi.spyOn(Element.prototype, "scrollHeight", "get").mockReturnValue(full);
  vi.spyOn(Element.prototype, "clientHeight", "get").mockReturnValue(shown);
}

function classes(element: HTMLElement): string[] {
  return element.className.split(" ");
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("ClampText", () => {
  it("shows a short text whole, with no toggle", () => {
    textHeight(36, 36);
    render(<ClampText>Own the web app.</ClampText>);
    expect(classes(screen.getByText("Own the web app."))).toContain(
      "line-clamp-3",
    );
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("opens and closes a cut text", async () => {
    textHeight(120, 36);
    render(<ClampText lines={2}>A long text.</ClampText>);
    const text = screen.getByText("A long text.");
    expect(classes(text)).toEqual(
      expect.arrayContaining(["line-clamp-2", "wrap-anywhere"]),
    );
    const more = screen.getByRole("button", { name: "More" });
    expect(more.getAttribute("aria-expanded")).toBe("false");
    expect(more.getAttribute("aria-controls")).toBe(text.id);

    await userEvent.click(more);
    expect(classes(text)).not.toContain("line-clamp-2");
    const less = screen.getByRole("button", { name: "Less" });
    expect(less.getAttribute("aria-expanded")).toBe("true");

    await userEvent.click(less);
    expect(classes(text)).toContain("line-clamp-2");
    screen.getByRole("button", { name: "More" });
  });
});
