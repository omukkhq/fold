import { findIds, type IdKind } from "@/format/ids";

export interface Segment {
  text: string;
  /** Set for a public id that has a screen; plain text has none. */
  href?: string;
}

function hrefOf(slug: string, kind: IdKind, id: string): string | undefined {
  const base = `/p/${slug}`;
  switch (kind) {
    case "task":
      return `${base}/t/${id}`;
    case "proposal":
      return `${base}/proposals/${id}`;
    case "changeset":
      return `${base}/diff?to=${id}`;
    case "revision":
      return `${base}/repository?revision=${id}`;
    case "message":
      return `${base}/messages?thread=${id}`;
    default:
      return undefined;
  }
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
