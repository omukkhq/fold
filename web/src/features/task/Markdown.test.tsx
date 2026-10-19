// @vitest-environment jsdom
import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Markdown } from "./Markdown";

afterEach(cleanup);

describe("Markdown code", () => {
  it("colours a fenced block whose fence names a language Shiki has, and keeps its text with the final newline", async () => {
    const { container } = render(
      <Markdown>{"```ts\nconst x = 1;\n```"}</Markdown>,
    );
    const code = container.querySelector("pre code");
    expect(code?.textContent).toBe("const x = 1;\n");
    await waitFor(() => {
      expect(code?.querySelector("span")).not.toBeNull();
    });
    expect(code?.textContent).toBe("const x = 1;\n");
    const keyword = [...(code?.querySelectorAll("span") ?? [])].find(
      (span) => span.textContent === "const",
    );
    expect(keyword?.style.color).toBe("var(--syntax-keyword)");
  });

  it("keeps an unknown fence, a bare fence and inline code plain", () => {
    const { container } = render(
      <Markdown>
        {"```brainfuck\n+[-]\n```\n\n```\nplain\n```\n\nRun `make check`."}
      </Markdown>,
    );
    const codes = [...container.querySelectorAll("code")];
    expect(codes.map((code) => code.textContent)).toEqual([
      "+[-]\n",
      "plain\n",
      "make check",
    ]);
    expect(container.querySelector("code span")).toBeNull();
  });

  it("draws inline code as inline code and keeps a fenced block's look", () => {
    const { container } = render(
      <Markdown>{"```\nfld record\n```\n\nRun `fld record` now."}</Markdown>,
    );
    const fenced = container.querySelector("pre code");
    expect(fenced?.className).toContain("text-meta");
    expect(fenced?.className).toContain("px-4");
    const inline = container.querySelector("p code");
    expect(inline?.textContent).toBe("fld record");
    expect(inline?.className).toContain("text-code");
    expect(inline?.className).toContain("px-5");
    expect(inline?.className).toContain("text-text");
    expect(inline?.className).not.toContain("text-meta");
  });
});
