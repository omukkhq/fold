const FIXTURE_IMPORT =
  /(?:from\s*|import\s*\(\s*|import\s+|require\s*\(\s*|import\.meta\.glob(?:<[^>]*>)?\s*\(\s*\[?\s*)["'][^"']*\bfixtures\//;

export function importsFixtures(source: string): boolean {
  return FIXTURE_IMPORT.test(source);
}

/** Fixtures belong to the `api/` seam; tests may read them too. */
export function mayImportFixtures(path: string): boolean {
  return path.includes("/src/api/") || /\.test\.tsx?$/.test(path);
}

const RETIRED_FIXTURE_IMPORT =
  /(?:from\s*|import\s*\(\s*|import\s+|require\s*\(\s*|import\.meta\.glob(?:<[^>]*>)?\s*\(\s*\[?\s*)["'][^"']*\bfixtures\/(?:board|demoBoard|diff|events|proposals|reviews|tasks|workspaces)["']/;

/** The board, task, proposal, live activity and diff screens read the daemon; their fixtures are gone and nothing, tests included, may bring them back. */
export function importsRetiredFixture(source: string): boolean {
  return RETIRED_FIXTURE_IMPORT.test(source);
}
