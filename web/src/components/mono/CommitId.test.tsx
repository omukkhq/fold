// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CommitId } from "./CommitId";

afterEach(cleanup);

const FULL = "4f2a9c1e4f2a9c1e4f2a9c1e4f2a9c1e4f2a9c1e";

describe("CommitId", () => {
  it("shows nothing for a commit that is not written yet", () => {
    const { container } = render(<CommitId commit={null} />);
    expect(container.firstChild).toBeNull();
  });

  it("shows the short id and copies the full one on click", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    render(<CommitId commit={FULL} />);
    const button = screen.getByRole("button", { name: "Copy commit 4f2a9c1" });
    expect(button.textContent).toContain("4f2a9c1");
    fireEvent.click(button);
    expect(await screen.findByText("Copied")).toBeTruthy();
    expect(writeText).toHaveBeenCalledWith(FULL);
  });

  it("is plain text inside another control", () => {
    render(<CommitId commit={FULL} copy={false} />);
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.getByText("4f2a9c1").className).toContain("font-mono");
  });
});
