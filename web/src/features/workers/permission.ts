import { apiErrorCode } from "@/api/errors";
import type { QuestionAnswer, WorkerQuestion } from "@/api/workers";
import { failureText } from "@/format/failure";
import type { Failure } from "./startLead";

/** How the panel shows a tool's input: the command or path itself where the tool has one, else the input as JSON. */
export interface InputView {
  text: string;
  /** Claude Code's own one-line description of a command. */
  note: string | null;
}

function field(input: Record<string, unknown>, key: string): string | null {
  const value = input[key];
  return typeof value === "string" && value !== "" ? value : null;
}

export function inputView(input: Record<string, unknown>): InputView {
  const command = field(input, "command");
  if (command !== null) {
    return { text: command, note: field(input, "description") };
  }
  const path = field(input, "file_path") ?? field(input, "path");
  if (path !== null) return { text: path, note: null };
  const url = field(input, "url");
  if (url !== null) return { text: url, note: null };
  return { text: JSON.stringify(input, null, 2), note: null };
}

export type ToolIcon = "terminal" | "file" | "globe" | "wrench";

const TOOL_ICONS: Record<string, ToolIcon> = {
  Bash: "terminal",
  Edit: "file",
  MultiEdit: "file",
  Write: "file",
  NotebookEdit: "file",
  WebFetch: "globe",
  WebSearch: "globe",
};

export function toolIcon(tool: string): ToolIcon {
  return TOOL_ICONS[tool] ?? "wrench";
}

/** `asks to run Bash` for a shell command, `asks to use Write` for any other tool. */
export function asksText(tool: string): string {
  return tool === "Bash" ? "asks to run Bash" : `asks to use ${tool}`;
}

/** Why the worker waits: Claude Code's permission rules asked for a human before the tool call ran. It names only the tool, never the call's input. */
export function whyAsksText(tool: string): string {
  return `Claude Code's permission rules asked for a human before the worker used ${tool}. The call runs only if you allow it.`;
}

export interface AnswerLine {
  allowed: boolean;
  lead: string;
  by: string;
  after: string;
}

/** `Allowed by @tanim` and what the worker does next. */
export function answerLine(answer: QuestionAnswer): AnswerLine {
  const allowed = answer.behavior === "allow";
  return {
    allowed,
    lead: allowed ? "Allowed by " : "Denied by ",
    by: answer.answered_by,
    after: allowed
      ? "the worker went on"
      : "the worker went on without the call",
  };
}

/**
 * The question the panel shows: the daemon's answer to this session's write wins, because an answered question leaves the open list.
 * Null when the id names no open question and no answer of this session.
 */
export function shownQuestion(
  open: readonly WorkerQuestion[],
  answered: WorkerQuestion | null,
  id: string,
): WorkerQuestion | null {
  if (answered !== null && answered.id === id) return answered;
  return open.find((q) => q.id === id) ?? null;
}

/** A refused answer, worded for its code. */
export function answerFailure(error: unknown): Failure {
  const code = apiErrorCode(error);
  switch (code) {
    case "permission_question_closed":
      return {
        code,
        text: "The question is closed: it has its answer, or fold stopped its worker.",
      };
    case "permission_question_not_found":
      return { code, text: "fold has no permission question with this id." };
    default:
      return { code, text: failureText(error) };
  }
}
