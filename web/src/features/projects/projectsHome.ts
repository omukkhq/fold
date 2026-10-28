import { apiErrorCode } from "@/api/errors";
import type { ProjectSummary } from "@/api/projects";
import { failureText } from "@/format/failure";
import type { Failure } from "../workers/startLead";

/** Whether the project is in fold: a person has not removed it. */
export function inFold(project: ProjectSummary): boolean {
  return project.removed_at === null;
}

/** Whether `query` is part of the project's name, slug or folder, ignoring case. */
export function matchesSearch(project: ProjectSummary, query: string): boolean {
  const needle = query.trim().toLowerCase();
  return [project.name, project.slug, project.root].some((text) =>
    text.toLowerCase().includes(needle),
  );
}

/** The projects the page lists: without a search, those in fold; with one, every match, the removed projects and those whose folder is missing too. */
export function shownProjects(
  projects: readonly ProjectSummary[],
  query: string,
): ProjectSummary[] {
  if (query.trim() === "") {
    return projects.filter(inFold);
  }
  return projects.filter((project) => matchesSearch(project, query));
}

/** The word that marks a project fold cannot show as it is: removed, or what is missing at its folder; null for a project that is there. */
export function projectMark(project: ProjectSummary): string | null {
  if (!inFold(project)) {
    return "removed";
  }
  switch (project.folder) {
    case "folder_missing":
      return "folder missing";
    case "fold_missing":
      return ".fold missing";
    case "database_missing":
      return "database missing";
    case "present":
      return null;
  }
}

/** Whether the card shows its folder path: in search results, and for a project whose mark tells something is wrong with it. */
export function showsRoot(project: ProjectSummary, query: string): boolean {
  return query.trim() !== "" || projectMark(project) !== null;
}

/** What a person can do with the project from its card: remove a project in fold, and add back a removed one whose folder still holds its `.fold/`. */
export function projectActions(project: ProjectSummary): {
  remove: boolean;
  addBack: boolean;
} {
  return {
    remove: inFold(project),
    addBack: !inFold(project) && project.folder === "present",
  };
}

/** Why a removed project cannot come back from its card, for one that cannot; null for one that can, and for a project in fold. */
export function addBackBlocker(project: ProjectSummary): string | null {
  if (inFold(project) || project.folder === "present") {
    return null;
  }
  return project.folder === "folder_missing"
    ? "Put the folder back to add the project back."
    : "Run `fld init` in the folder to make a new project there.";
}

/** A refused removal, worded for its code. */
export function removeFailure(error: unknown): Failure {
  const code = apiErrorCode(error);
  if (code === "project_workers_running") {
    return {
      code,
      text: "Workers of this project are running. Pause the project's agents or stop its workers, then remove it.",
    };
  }
  return { code, text: failureText(error) };
}

/** A refused add back, worded for its code. */
export function addBackFailure(error: unknown): Failure {
  const code = apiErrorCode(error);
  if (code === "project_unavailable") {
    return {
      code,
      text: "The folder does not hold this project now, so it stays removed.",
    };
  }
  return { code, text: failureText(error) };
}

/** How many folders a short path keeps. */
const SHORT_FOLDERS = 2;

/** `…/programming/nobs` for `/home/tanim/programming/nobs`: the last folders of a long absolute path, which the screen pairs with the whole path. The daemon sends no home folder, so the path never starts with `~`. */
export function shortPath(path: string): string {
  const parts = path.split("/").filter((part) => part !== "");
  if (parts.length <= SHORT_FOLDERS + 1) {
    return path;
  }
  return `…/${parts.slice(-SHORT_FOLDERS).join("/")}`;
}

/** How many agents a project card names before it says how many more there are. */
const NAMED_AGENTS = 3;

export interface AgentsLine {
  /** The agents the card names, in order. */
  named: string[];
  /** `and 3 more`, or null when the card names every agent. */
  more: string | null;
  /** Every agent, `@a, @b and @c`, for a screen reader and the hover text. */
  all: string;
}

/** The connected agents of a project card as plain names: the first three, then how many more. */
export function agentsLine(names: readonly string[]): AgentsLine {
  const at = names.map((name) => `@${name}`);
  const all =
    at.length <= 1
      ? (at[0] ?? "")
      : `${at.slice(0, -1).join(", ")} and ${at.at(-1) ?? ""}`;
  const hidden = names.length - NAMED_AGENTS;
  return {
    named: names.slice(0, NAMED_AGENTS),
    more: hidden > 0 ? `and ${String(hidden)} more` : null,
    all,
  };
}
