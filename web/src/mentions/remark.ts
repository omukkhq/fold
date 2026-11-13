import type { AgentLinks } from "@/features/board/agentLink";
import type { People } from "@/people/names";
import {
  findMentions,
  isPerson,
  mentionHref,
  type Known,
  type Mention,
} from "./parse";

/** The part of an mdast node the plugin reads and writes. */
export interface MdNode {
  type: string;
  value?: string;
  url?: string;
  children?: MdNode[];
  data?: { hProperties?: Record<string, string> };
}

/** Text under these nodes is never a mention: it is code, raw HTML, or already a link. */
const OPAQUE = new Set([
  "code",
  "inlineCode",
  "html",
  "link",
  "linkReference",
  "definition",
]);

/** The value the rendered link carries in `data-mention` (`dataMention` in hast), so the link component draws a mention chip. */
export function mentionTag(mention: Mention): string {
  if (isPerson(mention)) return mention.person;
  return mention.mark === "@" ? "path" : mention.kind;
}

interface MentionOptions {
  slug: string;
  known?: Known;
  people?: People;
  /** Where an agent's name leads; without it every agent opens the Agents screen. */
  links?: AgentLinks;
}

function splitText(value: string, options: MentionOptions): MdNode[] {
  const nodes: MdNode[] = [];
  let at = 0;
  for (const mention of findMentions(value, options.people)) {
    if (options.known && !options.known(mention)) continue;
    if (mention.start > at) {
      nodes.push({ type: "text", value: value.slice(at, mention.start) });
    }
    nodes.push({
      type: "link",
      url: mentionHref(options.slug, mention, options.links) ?? "",
      children: [{ type: "text", value: mention.text }],
      data: { hProperties: { dataMention: mentionTag(mention) } },
    });
    at = mention.end;
  }
  if (nodes.length === 0) return [{ type: "text", value }];
  if (at < value.length) nodes.push({ type: "text", value: value.slice(at) });
  return nodes;
}

function walk(node: MdNode, options: MentionOptions): void {
  if (!node.children || OPAQUE.has(node.type)) return;
  node.children = node.children.flatMap((child) => {
    if (child.type === "text" && typeof child.value === "string") {
      return splitText(child.value, options);
    }
    walk(child, options);
    return [child];
  });
}

/** A remark plugin that turns `#` and `@` mentions in Markdown text into links to their screens. Code spans and blocks are separate nodes, so a mention there stays text. */
export function remarkMentions(options: MentionOptions) {
  return () => (tree: MdNode) => {
    walk(tree, options);
  };
}
