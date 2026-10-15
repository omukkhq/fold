import { describe, expect, it } from "vitest";
import { knownFrom } from "./parse";
import { remarkMentions, type MdNode } from "./remark";

function run(tree: MdNode, known?: Parameters<typeof knownFrom>[0]): MdNode {
  const transform = remarkMentions({
    slug: "demo",
    known: known ? knownFrom(known) : undefined,
  })();
  transform(tree);
  return tree;
}

const text = (value: string): MdNode => ({ type: "text", value });

describe("remarkMentions", () => {
  it("turns mentions in text into links tagged with their kind", () => {
    const tree = run({
      type: "root",
      children: [
        { type: "paragraph", children: [text("Reads #T-9, in @web/a.ts.")] },
      ],
    });
    expect(tree.children?.[0]?.children).toEqual([
      text("Reads "),
      {
        type: "link",
        url: "/p/demo/t/T-9",
        children: [text("#T-9")],
        data: { hProperties: { dataMention: "task" } },
      },
      text(", in "),
      {
        type: "link",
        url: "/p/demo/repository?path=web%2Fa.ts",
        children: [text("@web/a.ts")],
        data: { hProperties: { dataMention: "path" } },
      },
      text("."),
    ]);
  });

  it("leaves code, links and text without mentions alone", () => {
    const code: MdNode = { type: "inlineCode", value: "#T-9" };
    const link: MdNode = {
      type: "link",
      url: "x",
      children: [text("#T-9")],
    };
    const tree = run({
      type: "root",
      children: [
        { type: "paragraph", children: [code, link, text("plain")] },
        { type: "code", value: "#T-1" },
      ],
    });
    expect(tree.children?.[0]?.children).toEqual([code, link, text("plain")]);
  });

  it("keeps a request the screen does not know as text", () => {
    const tree = run(
      {
        type: "root",
        children: [{ type: "paragraph", children: [text("#RQ-9 #RQ-1")] }],
      },
      { requests: ["RQ-1"] },
    );
    const nodes = tree.children?.[0]?.children ?? [];
    expect(nodes.map((n) => n.type)).toEqual(["text", "link"]);
    expect(nodes[0]).toEqual(text("#RQ-9 "));
  });
});
