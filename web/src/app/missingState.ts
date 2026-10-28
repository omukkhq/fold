import { projectPath } from "@/routes/paths";

/** What a not-found inside an open project names: an item by its id, or a page the project does not have. */
export type Missing =
  | { kind: "task" | "proposal" | "request" | "batch"; id: string }
  | { kind: "page" };

export interface MissingState {
  title: string;
  text: string;
  /** The daemon's code when the state stands for its refusal; null for an address fold refuses before it asks the daemon. */
  code: string | null;
  href: string;
  linkText: string;
}

const NOUNS = {
  task: "task",
  proposal: "proposal",
  request: "request",
  batch: "review batch",
} as const;

const TITLES = {
  task: "Task",
  proposal: "Proposal",
  request: "Request",
  batch: "Batch",
} as const;

/** The not-found state inside an open project. A batch leads back to the proposals, where its members are found; everything else leads back to the board. */
export function missingState(slug: string, missing: Missing): MissingState {
  if (missing.kind === "page") {
    return {
      title: "Page not found",
      text: "This project has no page at this address. Open the board to continue.",
      code: null,
      href: projectPath(slug, "board"),
      linkText: "Back to board",
    };
  }
  const where =
    "The id may be mistyped, or the link may come from another project.";
  if (missing.kind === "batch") {
    return {
      title: `${TITLES.batch} ${missing.id} not found`,
      text: `This project has no ${NOUNS.batch} with this id. ${where} Open the proposals to find the batch.`,
      code: "review_batch_not_found",
      href: projectPath(slug, "proposals"),
      linkText: "Back to proposals",
    };
  }
  return {
    title: `${TITLES[missing.kind]} ${missing.id} not found`,
    text: `This project has no ${NOUNS[missing.kind]} with this id. ${where} Open the board to continue.`,
    code: null,
    href: projectPath(slug, "board"),
    linkText: "Back to board",
  };
}
