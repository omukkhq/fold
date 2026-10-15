/** What an item or file chip shows: the mention as written without its `#` or `@` mark, because the chip's icon says what it names. */
export function chipLabel(text: string): string {
  return text.startsWith("#") || text.startsWith("@") ? text.slice(1) : text;
}
