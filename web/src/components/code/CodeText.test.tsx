// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { CodeText } from "./CodeText";

describe("CodeText", () => {
  it("draws each marked command as inline code and keeps the sentence's plain words", () => {
    const { container } = render(
      <p>
        <CodeText text="Run `fld doctor` to see why." />
      </p>,
    );
    const code = screen.getByText("fld doctor");
    expect(code.tagName).toBe("CODE");
    expect(code.className).toContain("text-13");
    expect(container.textContent).toBe("Run fld doctor to see why.");
  });
  it("takes the caption size inside caption text", () => {
    render(<CodeText text="`fld record`" caption />);
    expect(screen.getByText("fld record").className).toContain("text-13");
  });
});
