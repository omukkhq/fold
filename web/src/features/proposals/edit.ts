import type { ComparedFile } from "@/api/diff";
import type {
  Intent,
  NewEdit,
  ProposalDetail,
  VersionFile,
} from "@/api/proposals";
import { versionIntent } from "./intent";
import { intentOf, sameIntent, type IntentDraft } from "./intentDraft";
import type { DetailView } from "./view";

/**
 * A human changes a proposal only in a human review: an open proposal of a task with the human review mark, for the human the mark names, or for any human when it names none.
 * The daemon decides again on every write and refuses with `human_edit_denied`.
 */
export function inHumanReview(
  proposal: ProposalDetail,
  human: string | null,
): human is string {
  const mark = proposal.summary.human_review;
  return (
    human !== null &&
    proposal.summary.state === "open" &&
    mark !== null &&
    (mark.reviewer === null || mark.reviewer === human)
  );
}

/** The edit box opens only on the current version, the one a new version is made from. */
export function canEdit(
  proposal: ProposalDetail,
  human: string | null,
  view: Pick<DetailView, "current">,
): boolean {
  return view.current && inHumanReview(proposal, human);
}

/** A file of the diff the edit box can open: one the version still has, with text content. */
export function editableFile(file: ComparedFile | null): file is ComparedFile {
  return file !== null && file.change !== "deleted" && !file.binary;
}

export type LineEnding = "\n" | "\r\n";

/** What the edit box starts from: the file's text with its line ending, or why it cannot be edited. */
export type EditSource =
  | { kind: "text"; text: string; lineEnding: LineEnding }
  | { kind: "blocked"; reason: string };

/**
 * A text area holds `\n` line ends only, so a file written with `\r\n` is shown with `\n` and saved with `\r\n` again; otherwise one changed line would change every line of the file.
 */
export function editSource(file: VersionFile): EditSource {
  if (file.kind === "symlink" || file.kind === "directory") {
    return {
      kind: "blocked",
      reason: "This path is not a regular file, so it cannot be edited here.",
    };
  }
  if (file.too_large) {
    return {
      kind: "blocked",
      reason: "The file is larger than 1 MiB, so it cannot be edited here.",
    };
  }
  const text = file.text;
  if (file.binary || typeof text !== "string") {
    return {
      kind: "blocked",
      reason: "The file is binary, so it cannot be edited here.",
    };
  }
  const lineEnding: LineEnding = text.includes("\r\n") ? "\r\n" : "\n";
  return {
    kind: "text",
    text: lineEnding === "\r\n" ? text.replace(/\r\n/g, "\n") : text,
    lineEnding,
  };
}

/** The content the daemon stores for the text area's `value`, in the file's own line ending. */
export function savedContent(value: string, lineEnding: LineEnding): string {
  return lineEnding === "\r\n" ? value.replace(/\r?\n/g, "\r\n") : value;
}

/** Whether `value` differs from the file as the edit box opened it; Save needs a change. */
export function changed(
  source: Extract<EditSource, { kind: "text" }>,
  value: string,
): boolean {
  return value !== source.text;
}

/** The edit of `path` on the version on screen, never the current version the cache holds at the click, so a newer version makes the daemon refuse with `stale_version`. The intent goes with it when the version has none to keep (`started` null) or the human changed it; otherwise the new version keeps the edited version's intent. */
export function editRequest(
  view: Pick<DetailView, "version">,
  path: string,
  value: string,
  source: Extract<EditSource, { kind: "text" }>,
  draft: IntentDraft,
  started: Intent | null,
): NewEdit {
  const written = intentOf(draft);
  return {
    version: view.version,
    path,
    content: savedContent(value, source.lineEnding),
    ...(started !== null && sameIntent(written, started)
      ? {}
      : { intent: written }),
  };
}

/** The intent the edit form starts with: the edited version's, so the human changes what the edit changes instead of writing it again; null for a version without one. */
export function editIntent(
  proposal: ProposalDetail,
  version: number,
): Intent | null {
  const edited = proposal.versions.find((v) => v.version === version);
  return edited === undefined ? null : versionIntent(edited);
}

/** The hint over the intent form: what an intent says, and where the form's text came from. */
export function intentHint(version: number, started: Intent | null): string {
  const next = `v${String(version + 1)}`;
  const what = `How ${next} works in the code, and its parts: every file ${next} changes belongs to exactly one part.`;
  return started === null
    ? `${what} v${String(version)} has no intent, so write one for ${next}.`
    : `${what} It starts as the intent of v${String(version)}; change what your edit changes.`;
}

/** The line over the edit box: what a save writes, and that the approvals stay. */
export function editNotice(version: number): string {
  const v = `v${String(version)}`;
  const next = `v${String(version + 1)}`;
  return `You are editing ${v}. Saving writes ${next} under your name; ${v} stays in the history. ${next} keeps the agent approvals of ${v}, and you approve ${next} again.`;
}

/** The current version is a human edit whose kept approvals still count, so the human can send it for review again. */
export function canSendForReviewAgain(
  proposal: ProposalDetail,
  human: string | null,
): boolean {
  if (!inHumanReview(proposal, human)) return false;
  const current = proposal.versions.find(
    (v) => v.version === proposal.summary.current_version,
  );
  return current !== undefined && current.human_edit && !current.review_again;
}

/** The line about a human edit's approvals on the side panel; null for a version no human edited. */
export function keptApprovalsNote(
  proposal: ProposalDetail,
  version: number,
): string | null {
  const shown = proposal.versions.find((v) => v.version === version);
  if (!shown?.human_edit) return null;
  const from = `v${String(version - 1)}`;
  return shown.review_again
    ? `v${String(version)} is a human edit that was sent for review again, so only new reviews count.`
    : `v${String(version)} is a human edit. It keeps the agent approvals of ${from} until it is sent for review again.`;
}
