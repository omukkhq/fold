export type NavId =
  | "board"
  | "activity"
  | "proposals"
  | "requests"
  | "agents"
  | "messages"
  | "repository";

export interface NavEntry {
  id: NavId;
  label: string;
  segment: string;
}

const BOARD: NavEntry = { id: "board", label: "Board", segment: "board" };

export const NAV_ENTRIES: readonly NavEntry[] = [
  BOARD,
  { id: "activity", label: "Activity", segment: "activity" },
  { id: "proposals", label: "Proposals", segment: "proposals" },
  { id: "requests", label: "Requests", segment: "requests" },
  { id: "agents", label: "Agents", segment: "agents" },
  { id: "messages", label: "Messages", segment: "messages" },
  { id: "repository", label: "Repository", segment: "repository" },
];

export function navPath(slug: string, entry: NavEntry): string {
  return `/p/${slug}/${entry.segment}`;
}

/** The task dialog route renders the board beneath it, so `t/...` belongs to Board. */
const SEGMENT_OWNER: Readonly<Record<string, NavId>> = {
  t: "board",
};

/** The entry that owns a `/p/<slug>/...` path; the diff and bare project paths belong to none. */
export function activeNav(pathname: string): NavId | null {
  const [root, , segment] = pathname.split("/").filter(Boolean);
  if (root !== "p" || segment === undefined) {
    return null;
  }
  const owner = SEGMENT_OWNER[segment];
  if (owner) {
    return owner;
  }
  return NAV_ENTRIES.find((entry) => entry.segment === segment)?.id ?? null;
}

/** Switching project keeps the current screen; a path no entry owns lands on the board. */
export function switchProjectPath(pathname: string, slug: string): string {
  const active = activeNav(pathname);
  const entry = NAV_ENTRIES.find((e) => e.id === active) ?? BOARD;
  return navPath(slug, entry);
}
