import { ApiError, apiErrorCode } from "@/api/errors";
import { commonText } from "@/format/failure";
import { cyclePath, cycleText } from "./dependencies";

/** The form field a refusal belongs to. */
export type ErrorField = "name" | "color";

export interface ErrorView {
  /** The field that shows the text; null for a banner. */
  field: ErrorField | null;
  text: string;
}

function reasonOf(error: unknown): string | null {
  if (!(error instanceof ApiError)) {
    return null;
  }
  const { reason } = error.details;
  return typeof reason === "string" ? reason : null;
}

const INVALID_REQUEST: Record<string, string> = {
  self_dependency: "A task cannot depend on itself.",
  checklist_origin:
    "This dependency comes from a linked checklist item. Remove the item to remove it.",
  linked_item:
    "fold ticks a linked item when its task is Done. Change the linked task instead.",
  proposal_open:
    "A proposal is open for this task. Close it before you archive the task.",
};

const TASK_CLOSED: Record<string, string> = {
  done: "The task is Done and takes no more changes.",
  archived: "The task is archived and takes no more changes.",
};

/**
 * How a refused write is shown: a banner, or the text of a form field. Null for a code with no row, which the
 * generic `ErrorBanner` shows with the daemon's own message. Every row branches on `code`, and on `details.reason`
 * where one code covers several refusals; never on the message.
 */
export function describeError(error: unknown): ErrorView | null {
  const reason = reasonOf(error);
  switch (apiErrorCode(error)) {
    case "dependency_cycle": {
      const cycle = cyclePath(error);
      return {
        field: null,
        text: cycle ? cycleText(cycle) : "The dependency would close a cycle.",
      };
    }
    case "dependency_exists":
      return { field: null, text: "The task already needs that task." };
    case "label_exists":
      return { field: "name", text: "A label with this name exists." };
    case "invalid_label_color":
      return { field: "color", text: "Choose one of the eight colours." };
    case "invalid_request":
      return reason !== null && reason in INVALID_REQUEST
        ? { field: null, text: INVALID_REQUEST[reason] ?? "" }
        : null;
    case "task_not_reported":
      return {
        field: null,
        text: "No report waits for a check. The task worker reports the task again first.",
      };
    case "prerequisites_not_done":
      return {
        field: null,
        text: "A task that this task needs is not Done. Complete this task when its prerequisites are Done.",
      };
    case "task_kind_mismatch":
      return {
        field: null,
        text: "This task ends with a proposal, so its review completes it.",
      };
    case "invalid_hold_reason":
      return {
        field: null,
        text: "Give the blocker a reason of 1 to 500 characters.",
      };
    case "task_closed":
      return {
        field: null,
        text:
          (reason !== null ? TASK_CLOSED[reason] : undefined) ??
          "The task is closed and takes no more changes.",
      };
    default: {
      const text = commonText(apiErrorCode(error) ?? "");
      return text === undefined ? null : { field: null, text };
    }
  }
}

/** The text for one form field, or undefined when the refusal belongs elsewhere. */
export function fieldError(
  error: unknown,
  field: ErrorField,
): string | undefined {
  const view = describeError(error);
  return view?.field === field ? view.text : undefined;
}
