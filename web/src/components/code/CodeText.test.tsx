// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { CodeText, InlineCode } from "./CodeText";

afterEach(() => {
  cleanup();
});

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
  it("keeps a command in one chip that never breaks across lines, and draws it on the plain surface inside a sunken note", () => {
    const { container } = render(
      <p>
        <CodeText
          text="Fix the conflict markers and run `fld record`, or reset the folder."
          onSunken
        />
      </p>,
    );
    const code = screen.getByText("fld record");
    expect(code.className).toContain("whitespace-nowrap");
    expect(code.className).toContain("bg-surface");
    expect(code.className).not.toContain("bg-sunken");
    expect(container.textContent).toBe(
      "Fix the conflict markers and run fld record, or reset the folder.",
    );
    expect(code.nextSibling?.textContent?.startsWith(",")).toBe(true);
  });
  it("draws a command on the sunken surface by default", () => {
    render(<CodeText text="`fld doctor`" />);
    const code = screen.getByText("fld doctor");
    expect(code.className).toContain("bg-sunken");
    expect(code.className).toContain("whitespace-nowrap");
  });
  it("lets a wrapping chip break inside a long path and keeps its padding on each piece", () => {
    render(<InlineCode wrap>web/src/features/board/details.ts</InlineCode>);
    const code = screen.getByText("web/src/features/board/details.ts");
    expect(code.className).toContain("wrap-anywhere");
    expect(code.className).toContain("box-decoration-clone");
    expect(code.className).not.toContain("whitespace-nowrap");
  });
});
