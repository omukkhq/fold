// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { SyntaxText } from "./SyntaxText";

afterEach(cleanup);

function code(
  text: string,
  language: Parameters<typeof SyntaxText>[0]["language"],
) {
  render(
    <pre data-testid="code">
      <SyntaxText code={text} language={language} />
    </pre>,
  );
  return screen.getByTestId("code");
}

describe("SyntaxText", () => {
  it("shows the code as plain text at once and coloured once it is tokenized, with the same text", async () => {
    const text = "fn main() {\n    let x = 1;\n}";
    const pre = code(text, "fence:rust");
    expect(pre.textContent).toBe(text);
    await waitFor(() => {
      expect(pre.querySelector("span")).not.toBeNull();
    });
    expect(pre.textContent).toBe(text);
    const keyword = [...pre.querySelectorAll("span")].find(
      (span) => span.textContent === "fn",
    );
    expect(keyword?.style.color).toBe("var(--syntax-keyword)");
  });

  it("keeps code without a language plain", () => {
    const pre = code("let x = 1;", null);
    expect(pre.textContent).toBe("let x = 1;");
    expect(pre.querySelector("span")).toBeNull();
  });
});
