import { projectPath } from "@/routes/paths";

/** What a not-found inside an open project names: an item by its id, or a page the project does not have. */
export type Missing =
  | { kind: "task" | "proposal" | "request" | "batch"; id: string }
  | { kind: "page" };

export interface MissingState {
  title: string;
  text: string;
  href: string;
  linkText: string;
}

/** The noun of each kind and the screen its back link opens. A batch leads to the proposals, where its members are found. */
const KINDS = {
  task: { title: "Task", noun: "task", list: "board" },
  proposal: { title: "Proposal", noun: "proposal", list: "proposals" },
  request: { title: "Request", noun: "request", list: "requests" },
  batch: { title: "Batch", noun: "review batch", list: "proposals" },
} as const;

/** The not-found state inside an open project, for a malformed id and for an id the daemon does not have alike. Each item leads back to the list where it is found; a task and a page lead back to the board. */
export function missingState(slug: string, missing: Missing): MissingState {
  if (missing.kind === "page") {
    return {
      title: "Page not found",
      text: "This project has no page at this address. Open the board to continue.",
      href: projectPath(slug, "board"),
      linkText: "Back to board",
    };
  }
  const kind = KINDS[missing.kind];
  const next =
    missing.kind === "task"
      ? "Open the board to continue."
      : `Open the ${kind.list} to find the ${kind.noun}.`;
  return {
    title: `${kind.title} ${missing.id} not found`,
    text: `This project has no ${kind.noun} with this id. The id may be mistyped, or the link may come from another project. ${next}`,
    href: projectPath(slug, kind.list),
    linkText: `Back to ${kind.list}`,
  };
}
