import { atName, reviewerActor } from "./names";

/** A line of text that names people: plain text and actors, so the screen draws each actor as a coloured `@name` while a hint or a label can still take the plain text. */
export type Segment = string | { actor: string };

export type Segments = readonly Segment[];

export function actorSegment(actor: string): Segment {
  return { actor };
}

/** `a, b and c` over any segments. */
export function joinSegments(items: readonly Segments[]): Segment[] {
  return items.flatMap((item, index) => {
    if (index === 0) return [...item];
    const sep = index === items.length - 1 ? " and " : ", ";
    return [sep, ...item];
  });
}

// `Array.isArray` does not narrow a readonly array out of a union.
function isSegments(value: Segment | Segments): value is Segments {
  return Array.isArray(value);
}

/** Builds segments from a template; a value is a string, a segment or a list of segments. Adjacent strings merge. */
export function seg(
  strings: TemplateStringsArray,
  ...values: (string | number | Segment | Segments)[]
): Segment[] {
  const out: Segment[] = [];
  const push = (part: Segment) => {
    const last = out.at(-1);
    if (typeof part === "string" && typeof last === "string") {
      out[out.length - 1] = last + part;
    } else if (part !== "") {
      out.push(part);
    }
  };
  strings.forEach((text, index) => {
    push(text);
    const value = values[index];
    if (value === undefined) return;
    if (typeof value === "number") push(String(value));
    else if (isSegments(value)) {
      for (const part of value) push(part);
    } else push(value);
  });
  return out;
}

/** The line as plain text: an actor shows as `@name`. */
export function segmentsText(segments: Segments): string {
  return segments
    .map((part) => (typeof part === "string" ? part : atName(part.actor)))
    .join("");
}

/** The reviewer a human-review mark names, or the words for no one in particular. */
export function reviewerSegment(
  reviewer: string | null,
  none: string,
): Segment {
  const actor = reviewerActor(reviewer);
  return actor === null ? none : actorSegment(actor);
}
