import type { MdNode } from "@/mentions/remark";
import { linkify } from "./linkify";

/** Text under these nodes is never an id: it is code, raw HTML, or already a link such as a `#` mention. */
const OPAQUE = new Set([
  "code",
  "inlineCode",
  "html",
  "link",
  "linkReference",
  "definition",
]);

function walk(node: MdNode, slug: string): void {
  if (!node.children || OPAQUE.has(node.type)) return;
  node.children = node.children.flatMap((child): MdNode[] => {
    if (child.type !== "text" || typeof child.value !== "string") {
      walk(child, slug);
      return [child];
    }
    return linkify(child.value, slug).map((segment) =>
      segment.href === undefined
        ? { type: "text", value: segment.text }
        : {
            type: "link",
            url: segment.href,
            children: [{ type: "text", value: segment.text }],
            data: { hProperties: { dataId: "" } },
          },
    );
  });
}

/** A remark plugin that links the bare public ids of a message, such as P-3 without its `#`, to their screens. */
export function remarkIds(slug: string) {
  return () => (tree: MdNode) => {
    walk(tree, slug);
  };
}
