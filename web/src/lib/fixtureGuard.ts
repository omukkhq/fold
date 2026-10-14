const FIXTURE_IMPORT =
  /(?:from\s*|import\s*\(\s*|import\s+|require\s*\(\s*|import\.meta\.glob(?:<[^>]*>)?\s*\(\s*\[?\s*)["'][^"']*\bfixtures\//;

export function importsFixtures(source: string): boolean {
  return FIXTURE_IMPORT.test(source);
}

/** Fixtures belong to the `api/` seam; tests may read them too. */
export function mayImportFixtures(path: string): boolean {
  return path.includes("/src/api/") || /\.test\.tsx?$/.test(path);
}
