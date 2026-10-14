const MAX_LETTERS = 2;

/**
 * The letters an avatar shows. An agent name splits at `/`, `-`, `_` and `.` and takes the first letter
 * of the first two parts (`frontend/ui` is `FU`); a `human:` or `agent:` prefix is dropped (`human:tanim` is `T`).
 */
export function initials(name: string): string {
  const bare = name.replace(/^[A-Za-z]+:/, "");
  const letters = bare
    .split(/[^\p{L}\p{N}]+/u)
    .filter((part) => part.length > 0)
    .slice(0, MAX_LETTERS)
    .map((part) => (Array.from(part)[0] ?? "").toUpperCase());
  return letters.join("") || "?";
}
