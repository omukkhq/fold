const FIXTURE_IMPORT =
  /(?:from\s*|import\s*\(\s*|import\s+|require\s*\(\s*|import\.meta\.glob(?:<[^>]*>)?\s*\(\s*\[?(?:\s*["'][^"']*["']\s*,)*\s*)["'](?:[^"']*\/)?fixtures(?:\/[^"']*)?["']/;

/** Screens read the daemon and nothing ships demo data, so no source, tests included, imports a `fixtures/` path. */
export function importsFixtures(source: string): boolean {
  return FIXTURE_IMPORT.test(source);
}
