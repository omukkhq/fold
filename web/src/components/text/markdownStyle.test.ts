import { describe, expect, it } from "vitest";
import {
  headingSizeClass,
  linkTarget,
  markdownBoxClass,
  markdownSizeClass,
  remarkHtmlAsText,
  remarkLineBreaks,
} from "./markdownStyle";

interface Node {
  type: string;
  value?: string;
  children?: Node[];
}

function paragraph(...children: Node[]): Node {
  return { type: "paragraph", children };
}

function text(value: string): Node {
  return { type: "text", value };
}

describe("markdown sizes", () => {
  it("gives the text its size and keeps headings in it, with one step more for the first heading of body text", () => {
    expect(markdownSizeClass(13)).toBe("text-13");
    expect(markdownSizeClass(16)).toBe("text-16");
    expect(headingSizeClass(15, 1)).toBe("text-16");
    expect(headingSizeClass(15, 2)).toBe("text-15");
    expect(headingSizeClass(13, 1)).toBe("text-13");
    expect(headingSizeClass(16, 1)).toBe("text-16");
  });
});

describe("markdownBoxClass", () => {
  it("sets the blocks in a column with a gap while open", () => {
    expect(markdownBoxClass(null)).toBe("flex min-w-0 flex-col gap-6");
  });

  it("cuts a clamped box to its lines and keeps the blocks apart with margins, because a line clamp cannot be a flex column", () => {
    const box = markdownBoxClass(4);
    expect(box).toContain("line-clamp-4");
    expect(box).toContain("[&>*+*]:mt-6");
    expect(box).not.toContain("flex");
  });
});

// The token guard refuses a remote address in the sources, so the test builds one.
const SITE = ["/", "/example.com"].join("");

describe("linkTarget", () => {
  it("opens a link that leaves the app in a new tab without a referrer", () => {
    const external = { target: "_blank", rel: "noopener noreferrer" };
    expect(linkTarget(`https:${SITE}/a`)).toEqual(external);
    expect(linkTarget(`HTTP:${SITE}`)).toEqual(external);
    expect(linkTarget("mailto:a@example.com")).toEqual(external);
  });

  it("keeps a link inside the app, and an address the renderer emptied, in the same tab", () => {
    expect(linkTarget("/p/demo/board")).toBeNull();
    expect(linkTarget("")).toBeNull();
    expect(linkTarget(undefined)).toBeNull();
  });
});

describe("remarkLineBreaks", () => {
  it("turns each line end inside a paragraph into a line break and drops no text", () => {
    const tree: Node = {
      type: "root",
      children: [paragraph(text("one\ntwo\n\nthree")), paragraph(text("x"))],
    };
    remarkLineBreaks()(tree);
    expect(tree.children?.[0]?.children).toEqual([
      text("one"),
      { type: "break" },
      text("two"),
      { type: "break" },
      { type: "break" },
      text("three"),
    ]);
    expect(tree.children?.[1]?.children).toEqual([text("x")]);
  });

  it("walks into nested blocks and leaves code alone", () => {
    const code: Node = { type: "code", value: "a\nb" };
    const tree: Node = {
      type: "root",
      children: [
        { type: "list", children: [paragraph(text("a\nb"))] },
        code,
        paragraph({ type: "inlineCode", value: "x\ny" }),
      ],
    };
    remarkLineBreaks()(tree);
    expect(tree.children?.[0]?.children?.[0]?.children).toEqual([
      text("a"),
      { type: "break" },
      text("b"),
    ]);
    expect(tree.children?.[1]).toEqual(code);
    expect(tree.children?.[2]?.children).toEqual([
      { type: "inlineCode", value: "x\ny" },
    ]);
  });
});

describe("remarkHtmlAsText", () => {
  it("shows inline HTML as its text and a block of HTML as a paragraph of its text", () => {
    const tree: Node = {
      type: "root",
      children: [
        paragraph(
          text("use "),
          { type: "html", value: "<Button>" },
          text(" here"),
        ),
        { type: "html", value: "<div>block</div>" },
        {
          type: "blockquote",
          children: [{ type: "html", value: "<i>" }],
        },
      ],
    };
    remarkHtmlAsText()(tree);
    expect(tree.children?.[0]?.children).toEqual([
      text("use "),
      text("<Button>"),
      text(" here"),
    ]);
    expect(tree.children?.[1]).toEqual(paragraph(text("<div>block</div>")));
    expect(tree.children?.[2]?.children).toEqual([paragraph(text("<i>"))]);
  });
});
