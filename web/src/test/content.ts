/** A getByText matcher for text that an `@name` splits across elements: the deepest element whose whole text content matches. */
export function byContent(
  expected: string | RegExp,
): (content: string, element: Element | null) => boolean {
  const matches = (element: Element) =>
    typeof expected === "string"
      ? element.textContent === expected
      : expected.test(element.textContent);
  return (_content, element) =>
    element !== null &&
    matches(element) &&
    !Array.from(element.children).some(matches);
}
