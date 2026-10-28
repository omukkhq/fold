import { ID_CLASSES } from "@/components/id/idStyles";

/** What an item or file chip shows: the mention as written without its `#` or `@` mark, because the chip's icon says what it names. */
export function chipLabel(text: string): string {
  return text.startsWith("#") || text.startsWith("@") ? text.slice(1) : text;
}

/** A file chip names a path, which reads in the mono face; every other chip names an item by its id, which reads in the body face with tabular numbers. */
export function chipFace(tag: string): string {
  return tag === "path" ? "font-mono text-13" : `${ID_CLASSES} text-14`;
}
