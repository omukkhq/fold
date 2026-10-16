import { findIds, type IdKind } from "@/format/ids";
import { idHref } from "@/routes/paths";

export interface Segment {
  text: string;
  /** Set for a public id that has a screen; plain text has none. */
  href?: string;
}

/** A workspace and a checkpoint have no screen of their own, so a message leaves them as text. */
function hrefOf(slug: string, kind: IdKind, id: string): string | undefined {
  return kind === "workspace" || kind === "checkpoint"
    ? undefined
    : idHref(slug, kind, id);
}

/** Splits a body into text and linked ids. The body stays text: the caller renders every segment as a text node. */
export function linkify(body: string, slug: string): Segment[] {
  const segments: Segment[] = [];
  let at = 0;
  for (const found of findIds(body)) {
    const href = hrefOf(slug, found.kind, found.text);
    if (href === undefined) {
      continue;
    }
    if (found.start > at) {
      segments.push({ text: body.slice(at, found.start) });
    }
    segments.push({ text: found.text, href });
    at = found.end;
  }
  if (at < body.length) {
    segments.push({ text: body.slice(at) });
  }
  return segments;
}
