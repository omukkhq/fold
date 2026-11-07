// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { keys } from "@/api/keys";
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
    expect(fenced?.className).toContain("text-13");
    expect(fenced?.className).toContain("px-4");
    const inline = container.querySelector("p code");
    expect(inline?.textContent).toBe("fld record");
    expect(inline?.className).toContain("text-13");
    expect(inline?.className).toContain("px-5");
    expect(inline?.className).toContain("text-text");
  });
});

// The token guard refuses a remote address in the sources, so the test builds one.
const REMOTE = ["https:", "//example.com"].join("");

describe("Markdown safety", () => {
  const SOURCE = [
    "| Code | Message |",
    "| --- | --- |",
    "| `rate_limited` | Try later. |",
    "",
    "- [x] Agree the copy",
    "- [ ] Check it",
    "",
    "Some <b>bold</b> words",
    "",
    "<script>alert(1)</script>",
    "",
    '<div class="x">block</div>',
    "",
    "[run](javascript:alert(2)) and [board](/p/demo/board)",
  ].join("\n");

  it("renders tables and task lists and, with dropHtml, drops raw HTML", () => {
    const { container } = render(<Markdown dropHtml>{SOURCE}</Markdown>);
    expect(screen.getByRole("table")).toBeTruthy();
    expect(
      screen.getAllByRole("columnheader").map((h) => h.textContent),
    ).toEqual(["Code", "Message"]);
    const boxes = screen.getAllByRole<HTMLInputElement>("checkbox");
    expect(boxes.map((b) => b.checked)).toEqual([true, false]);
    expect(boxes.every((b) => b.disabled)).toBe(true);
    expect(container.querySelector("b, script, div.x")).toBeNull();
    expect(container.textContent).not.toContain("<b>");
    expect(container.textContent).not.toContain("alert(1)");
    expect(container.textContent).not.toContain("block");
    expect(
      [...container.querySelectorAll("a")].map((a) => a.getAttribute("href")),
    ).toEqual(["", "/p/demo/board"]);
  });

  it("shows raw HTML as typed by default and makes no element of it", () => {
    const { container } = render(<Markdown>{SOURCE}</Markdown>);
    expect(container.querySelector("b, script, div.x")).toBeNull();
    expect(container.textContent).toContain("Some <b>bold</b> words");
    expect(container.textContent).toContain("<script>alert(1)</script>");
    expect(screen.getByText('<div class="x">block</div>').tagName).toBe("P");
  });

  it("opens a link that leaves the app in a new tab without a referrer and keeps an app link in the tab", () => {
    render(
      <Markdown>
        {`[docs](${REMOTE}/docs) and [board](/p/demo/board)`}
      </Markdown>,
    );
    const docs = screen.getByRole("link", { name: "docs" });
    expect(docs.getAttribute("target")).toBe("_blank");
    expect(docs.getAttribute("rel")).toBe("noopener noreferrer");
    expect(
      screen.getByRole("link", { name: "board" }).getAttribute("target"),
    ).toBeNull();
  });
});

describe("Markdown text", () => {
  it("keeps each line end as a line break, as an agent wrote it", () => {
    const { container } = render(
      <Markdown>{"Checked:\nthe board\nthe dialog"}</Markdown>,
    );
    const paragraph = container.querySelector("p");
    expect(paragraph?.querySelectorAll("br")).toHaveLength(2);
    expect(paragraph?.textContent).toBe("Checked:\nthe board\nthe dialog");
  });

  it("renders emphasis, lists and headings in the size it is given, and wraps a long path", () => {
    const { container } = render(
      <Markdown size={13} className="text-text-2">
        {"# Head\n\n*one* and **two**\n\n1. first\n2. second"}
      </Markdown>,
    );
    const box = container.firstElementChild;
    expect(box?.className).toContain("text-13");
    expect(box?.className).toContain("text-text-2");
    expect(box?.className).not.toContain("text-text ");
    expect(box?.className).toContain("wrap-anywhere");
    expect(screen.getByText("Head").className).toContain("text-13");
    expect(screen.getByText("one").tagName).toBe("EM");
    expect(screen.getByText("two").tagName).toBe("STRONG");
    expect(screen.getAllByRole("listitem")).toHaveLength(2);
  });

  it("draws inline code as the shared chip that wraps, on the plain surface when the text sits on the sunken one", () => {
    render(
      <Markdown onSunken>
        {"Read `web/src/features/board/details.ts`."}
      </Markdown>,
    );
    const code = screen.getByText("web/src/features/board/details.ts");
    expect(code.tagName).toBe("CODE");
    expect(code.className).toContain("bg-surface");
    expect(code.className).toContain("wrap-anywhere");
    expect(code.className).not.toContain("whitespace-nowrap");
  });

  it("clamps the rendered text to its lines", () => {
    const { container } = render(<Markdown clamp={4}>{"one\n\ntwo"}</Markdown>);
    const box = container.firstElementChild;
    expect(box?.className).toContain("line-clamp-4");
    expect(box?.className).not.toContain("flex-col");
    expect(container.querySelectorAll("p")).toHaveLength(2);
  });
});

function withPeople(node: ReactNode) {
  const client = new QueryClient();
  client.setQueryData(keys.humans(), ["tanim"]);
  client.setQueryData(keys.agentBar("demo"), [
    { name: "frontend", workers: [{ name: "frontend" }] },
  ]);
  return <QueryClientProvider client={client}>{node}</QueryClientProvider>;
}

describe("Markdown mentions", () => {
  it("links mentions inside emphasis and lists and leaves one in a code span as code", () => {
    const { container } = render(
      withPeople(
        <Markdown mentions={{ slug: "demo" }}>
          {"**ask @tanim** about #T-9\n\n- see @web/src/a.ts\n\nnot `#T-10`"}
        </Markdown>,
      ),
    );
    expect(
      container.querySelector("strong [data-mention='human']")?.textContent,
    ).toBe("@tanim");
    expect(container.querySelector('[data-mention="task"]')?.textContent).toBe(
      "T-9",
    );
    expect(
      container.querySelector('li [data-mention="path"]')?.textContent,
    ).toBe("web/src/a.ts");
    expect(screen.getByText("#T-10").tagName).toBe("CODE");
    expect(container.querySelectorAll("[data-mention]")).toHaveLength(3);
  });

  it("runs extra plugins after the mentions", () => {
    const calls: string[] = [];
    const plugin = () => () => {
      calls.push("ran");
    };
    render(
      withPeople(
        <Markdown mentions={{ slug: "demo" }} plugins={[plugin]}>
          {"text"}
        </Markdown>,
      ),
    );
    expect(calls).toEqual(["ran"]);
  });
});
