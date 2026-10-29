import { ApiError, apiErrorCode } from "@/api/errors";
import { commonText } from "@/format/failure";
import { atName } from "@/people/names";
import { updateRunsLine } from "./actions";

/** `inline` shows on the control whose write failed: the actions or the form. `banner` shows above the screen. */
export type FailurePlace = "inline" | "banner";

export interface Failure {
  place: FailurePlace;
  text: string;
  /** False for a code without an entry, which the generic banner shows with the server's message. */
  mapped: boolean;
}

type Details = Readonly<Record<string, unknown>>;

interface Entry {
  place: FailurePlace;
  text: string | ((details: Details) => string);
}

function count(details: Details, key: string): string | null {
  const value = details[key];
  return typeof value === "number" ? String(value) : null;
}

/** The task ids of `details.prerequisites`, each `{task, status}`. */
function unfinished(details: Details): string[] {
  const list = details.prerequisites;
  if (!Array.isArray(list)) return [];
  return list.flatMap((item: unknown) =>
    typeof item === "object" &&
    item !== null &&
    "task" in item &&
    typeof item.task === "string"
      ? [item.task]
      : [],
  );
}

const ENTRIES: Record<string, Entry> = {
  author_cannot_review: {
    place: "inline",
    text: "You wrote this proposal, so you cannot review it.",
  },
  author_cannot_integrate: {
    place: "inline",
    text: "Your agent wrote this proposal, so you cannot integrate it.",
  },
  stale_version: {
    place: "inline",
    text: (d) => {
      const current = count(d, "current_version");
      return current === null
        ? "A newer version arrived. Read it, then try once more."
        : `v${current} is now the current version. Read it, then try once more.`;
    },
  },
  agent_approval_required: {
    place: "inline",
    text: "A reviewer agent approves this version first, then you can approve it.",
  },
  not_approved: {
    place: "inline",
    text: "No approval counts for this version yet, so fold did not integrate it.",
  },
  changes_requested: {
    place: "inline",
    text: "The latest verdict on this version asks for changes, so fold did not integrate it.",
  },
  human_approval_missing: {
    place: "inline",
    text: (d) =>
      typeof d.reviewer === "string"
        ? `fold waits for ${atName(d.reviewer)} to approve this version.`
        : "fold waits for a human to approve this version.",
  },
  prerequisites_not_done: {
    place: "inline",
    text: (d) => {
      const tasks = unfinished(d);
      return tasks.length === 0
        ? "A prerequisite is not Done, so fold did not integrate."
        : `${tasks.join(", ")} ${tasks.length === 1 ? "is" : "are"} not Done, so fold did not integrate.`;
    },
  },
  integration_conflicted: {
    place: "inline",
    text: "The version conflicts with main, so fold did not integrate it. The conflict report is above.",
  },
  head_moved: {
    place: "inline",
    text: "Main moved during the integration. Try once more.",
  },
  integration_failed: {
    place: "inline",
    text: "The integration failed and main did not change. Try once more.",
  },
  proposal_not_open: {
    place: "inline",
    text: "The proposal is no longer open.",
  },
  task_closed: {
    place: "inline",
    text: "The task is Done or archived, so the proposal takes no review.",
  },
  empty_review: {
    place: "inline",
    text: "Write what you found first.",
  },
  unknown_comment_path: {
    place: "inline",
    text: "This version does not change that file.",
  },
  invalid_path: {
    place: "inline",
    text: "fold refused the file path as written.",
  },
  invalid_comment_range: {
    place: "inline",
    text: "These lines are not in the file on that side.",
  },
  invalid_request: {
    place: "inline",
    text: "fold refused the request as written.",
  },
  proposal_not_found: {
    place: "banner",
    text: "fold has no proposal with this id.",
  },
  human_edit_denied: {
    place: "inline",
    text: (d) =>
      typeof d.reviewer === "string"
        ? `Only ${atName(d.reviewer)} can change this proposal in the human review.`
        : "You can change a proposal only in a human review.",
  },
  worker_busy: {
    place: "inline",
    text: (d) =>
      d.reason === "working"
        ? "The task worker is working in its workspace now. Try again when it stops."
        : d.reason === "unfinished_write"
          ? "The task workspace waits for its worker to sync an earlier change. Try again after that."
          : "The task worker has work that this version does not hold. Ask for changes instead.",
  },
  file_not_editable: {
    place: "inline",
    text: (d) =>
      d.reason === "missing"
        ? "This version does not have that file."
        : d.reason === "too_large"
          ? "The file is too large to edit here."
          : "Only a text file can be edited here.",
  },
  update_checks_running: {
    place: "inline",
    text: (d) => {
      const fix = d.purpose === "review_fix";
      const running =
        typeof d.version === "number"
          ? updateRunsLine(d.version + 1, fix)
          : fix
            ? "A reviewer's fix is running its checks."
            : "The next version is running its checks.";
      return `${running} Try again when they end.`;
    },
  },
  version_unchanged: {
    place: "inline",
    text: "The file has no change yet. Change it, then save.",
  },
  invalid_intent: {
    place: "inline",
    text: (d) =>
      d.reason === "too_long"
        ? "The intent is too long. Keep it to 2000 characters."
        : "Say what the new version does, why, what it does not change and its risks.",
  },
};

/** Where and how the screen shows a failed write. It branches on the error code; a code every screen words the same way shows its shared sentence as a banner, and the message shows only for a code without an entry. */
export function describeFailure(error: unknown): Failure {
  const code = apiErrorCode(error);
  const common = code === null ? undefined : commonText(code);
  const entry: Entry | undefined =
    code === null
      ? undefined
      : (ENTRIES[code] ??
        (common === undefined ? undefined : { place: "banner", text: common }));
  if (!entry) {
    const message =
      error instanceof Error && error.message ? error.message : "";
    return {
      place: "banner",
      text: message || "The request did not reach the daemon. Try again.",
      mapped: false,
    };
  }
  const details: Details = error instanceof ApiError ? error.details : {};
  return {
    place: entry.place,
    text: typeof entry.text === "string" ? entry.text : entry.text(details),
    mapped: true,
  };
}

/** Codes after which the proposal on screen is out of date, so the screen reads it again. */
export function reloadsProposal(error: unknown): boolean {
  const code = apiErrorCode(error);
  return (
    code === "stale_version" ||
    code === "integration_conflicted" ||
    code === "proposal_not_open" ||
    code === "invalid_request" ||
    code === "update_checks_running"
  );
}
