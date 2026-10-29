import { ApiError, apiErrorCode } from "@/api/errors";
import { REQUEST_TOO_LARGE } from "@/api/requests";
import { commonText } from "@/format/failure";
import { MAX_TEXT_CHARS } from "@/format/text";

type Details = Readonly<Record<string, unknown>>;

export const MAX_CARDS = 20;
export const MAX_TITLE_CHARS = 200;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function str(details: Details, key: string, fallback: string): string {
  const value = details[key];
  return typeof value === "string" || typeof value === "number"
    ? String(value)
    : fallback;
}

/** The sentence for each `invalid_draft` reason, used for the daemon's refusal and for the same check before saving. */
export function draftReasonText(reason: string, details: Details): string {
  const key = str(details, "key", "a card");
  const position = str(details, "position", "?");
  switch (reason) {
    case "empty":
      return "A draft needs at least one card.";
    case "too_many_cards":
      return `A draft holds at most ${String(MAX_CARDS)} cards.`;
    case "invalid_key":
      return `Card ${position} has a key fold does not accept. Use 1 to 32 letters, digits, - or _.`;
    case "duplicate_key":
      return `Two cards use the key ${key}.`;
    case "empty_title":
      return `Card ${key} needs a title.`;
    case "title_not_one_line":
      return `The title of ${key} must be one line.`;
    case "title_too_long":
      return `The title of ${key} is longer than ${String(MAX_TITLE_CHARS)} characters.`;
    case "empty_description":
      return `Card ${key} needs a description.`;
    case "description_too_long":
      return `The description of ${key} is longer than ${String(MAX_TEXT_CHARS)} characters.`;
    case "unknown_key":
      return `Edge ${position} names a card that is not in the draft.`;
    case "self_edge":
      return `${key} cannot need itself.`;
    case "duplicate_edge": {
      const e = details.edge;
      if (isRecord(e)) {
        return `${str(e, "dependent", "A card")} already needs ${str(e, "prerequisite", "that card")}.`;
      }
      return "Two edges are the same.";
    }
    case "cycle":
      return cycleText(details);
    default:
      return "fold refused the draft as written.";
  }
}

function cycleText(details: Details): string {
  const path = details.path;
  if (Array.isArray(path) && path.every((k) => typeof k === "string")) {
    return `These edges make a cycle: ${path.join(" → ")}. Remove one of them.`;
  }
  return "The edges make a cycle. Remove one of them.";
}

const ATTACHMENT_REASONS: Record<string, (position: string) => string> = {
  too_many_attachments: () => "A request holds at most 10 files.",
  attachment_too_large: (p) => `File ${p} is larger than 10 MiB.`,
  invalid_attachment_name: (p) => `File ${p} has a name fold does not accept.`,
  invalid_media_type: (p) => `File ${p} has a type fold does not accept.`,
};

const TEXTS: Record<string, string | ((details: Details) => string)> = {
  requester_required: "Only the requester can do this.",
  request_not_open:
    "The request is approved or closed, so it takes no more changes.",
  request_not_found: "fold has no request with this id.",
  stale_version: (d) =>
    `v${str(d, "latest", "?")} is now the latest version. Read it, then try once more.`,
  invalid_draft: (d) => draftReasonText(str(d, "reason", ""), d),
  dependency_cycle: cycleText,
  invalid_request: (d) => {
    const reason = str(d, "reason", "");
    const attachment = ATTACHMENT_REASONS[reason];
    if (attachment) return attachment(str(d, "position", "?"));
    if (reason === "state") {
      return "The request moved on, so this action does not fit it now. Read it again.";
    }
    return "fold refused the request as written.";
  },
  [REQUEST_TOO_LARGE]:
    "The files are larger than fold takes in one request. Remove a file, then try once more.",
  unknown_comment_path: (d) => {
    switch (d.reason) {
      case "missing":
        return "That revision or version has no file at this path, so its lines take no request.";
      case "binary":
        return "This file is binary, so its lines take no request.";
      case "too_large":
        return "This file is too large for fold to read, so its lines take no request.";
      case "not_a_file":
        return "This path is not a file in that revision or version, so it takes no request.";
      default:
        return "This file is not a text file of that revision or version, so it takes no request on its lines.";
    }
  },
  invalid_comment_range:
    "These lines are not in the file now. Select the lines again.",
  revision_not_found: "fold has no such revision.",
  proposal_not_found: "fold has no such proposal or version.",
  invalid_path: "fold does not accept this file path.",
};

export interface Failure {
  text: string;
  /** The daemon's code, shown under the sentence; null when the request did not reach it. */
  code: string | null;
}

/** The sentence and code for a failed write. It branches on the code; the daemon's message shows only for a code without a sentence. */
export function describeFailure(error: unknown): Failure {
  const code = apiErrorCode(error);
  if (code === null) {
    return { text: "The request did not reach the daemon. Try again.", code };
  }
  const entry = TEXTS[code] ?? commonText(code);
  const details: Details = error instanceof ApiError ? error.details : {};
  if (entry === undefined) {
    const message =
      error instanceof Error && error.message ? error.message : "";
    return { text: message || "The daemon refused the request.", code };
  }
  return { text: typeof entry === "string" ? entry : entry(details), code };
}

/** Codes after which the request on screen is out of date, so the screen reads it again. */
export function reloadsRequest(error: unknown): boolean {
  const code = apiErrorCode(error);
  return (
    code === "stale_version" ||
    code === "request_not_open" ||
    (code === "invalid_request" &&
      error instanceof ApiError &&
      error.details.reason === "state")
  );
}
