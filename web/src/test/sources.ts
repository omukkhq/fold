import { readdirSync, readFileSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const SRC = fileURLToPath(new URL("..", import.meta.url));

/**
 * Reads the files under `src/<dir>` from disk, keyed `/src/<path>` as `import.meta.glob` keys them, and leaves out the `importer` (an `import.meta.url`) as the glob does.
 * The source guards read the disk because Vitest's module cache keeps an eager glob's file list from an earlier run, so a guard that globs misses a file added since.
 */
export function readSources(
  importer: string,
  dir: string,
  extensions: readonly string[],
  recursive = true,
): Record<string, string> {
  const self = fileURLToPath(importer);
  const sources: Record<string, string> = {};
  const entries = readdirSync(join(SRC, dir), {
    recursive,
    withFileTypes: true,
  });
  for (const entry of entries) {
    if (
      !entry.isFile() ||
      !extensions.some((ext) => entry.name.endsWith(`.${ext}`))
    ) {
      continue;
    }
    const path = join(entry.parentPath, entry.name);
    if (path === self) {
      continue;
    }
    const key = `/src/${relative(SRC, path).split(sep).join("/")}`;
    sources[key] = readFileSync(path, "utf8");
  }
  return sources;
}
