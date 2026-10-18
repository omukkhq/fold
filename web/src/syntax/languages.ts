/**
 * How code names its language: `file:` and the file's name, or `fence:` and the info word of a Markdown fence, both in
 * lower case. The catalog resolves it to a Shiki language in its own chunk, so the page's first load carries no table
 * of the languages.
 */
export type LanguageSource = `file:${string}` | `fence:${string}`;

/** The language of a file, named by its last path segment. */
export function languageForPath(path: string): LanguageSource {
  return `file:${path.slice(path.lastIndexOf("/") + 1).toLowerCase()}`;
}

/** The language a Markdown fence names, from the `language-<info>` class the Markdown renderer gives a fenced block; null for a bare fence. */
export function languageForFence(
  className: string | undefined,
): LanguageSource | null {
  const info = /(?:^|\s)language-(\S+)/.exec(className ?? "")?.[1];
  return info === undefined ? null : `fence:${info.toLowerCase()}`;
}
