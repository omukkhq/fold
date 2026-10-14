// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { DiffLine } from "./DiffLine";

afterEach(cleanup);

describe("DiffLine", () => {
  it("shows an added line with its sign, number and kind", () => {
    const { container } = render(
      <DiffLine kind="add" newNo={12}>
        let a = 1;
      </DiffLine>,
    );
    const row = container.firstElementChild;
    expect(row?.getAttribute("data-kind")).toBe("add");
    expect(row?.className).toContain("bg-diff-add");
    expect(screen.getByText("Added line")).toBeTruthy();
    expect(screen.getByText("+")).toBeTruthy();
    expect(screen.getByText("12").className).toContain("w-diff-number");
    expect(screen.getByText("let a = 1;").className).toContain("font-mono");
  });

  it("uses the selected fill and shows the marker", () => {
    const { container } = render(
      <DiffLine
        kind="remove"
        oldNo={3}
        selected
        marker={<button type="button">Comment</button>}
      >
        x
      </DiffLine>,
    );
    const row = container.firstElementChild;
    expect(row?.className).toContain("bg-diff-selected");
    expect(row?.hasAttribute("data-selected")).toBe(true);
    expect(screen.getByRole("button", { name: "Comment" })).toBeTruthy();
  });

  it("renders hunk and context kinds", () => {
    render(
      <>
        <DiffLine kind="hunk">@@ -1,3 +1,4 @@</DiffLine>
        <DiffLine kind="context" oldNo={1} newNo={1}>
          same
        </DiffLine>
      </>,
    );
    expect(screen.getByText("Hunk header")).toBeTruthy();
    expect(screen.getByText("@@ -1,3 +1,4 @@").className).toContain(
      "text-label",
    );
    expect(screen.getByText("Unchanged line")).toBeTruthy();
  });
});
