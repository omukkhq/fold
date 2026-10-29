import type {
  CodeRequestBody,
  RequestCode,
  RequestSummary,
} from "@/api/requests";
import type { DiffSelection, DiffThread } from "@/features/diff/selection";
import { linesText } from "@/features/proposals/threadsModel";
import { detailHref } from "@/features/proposals/view";
import { projectPath, withQuery } from "@/routes/paths";
import { isFinal } from "./actions";
import { STATE_LABELS } from "./present";

/** What the selected lines belong to: a revision of main, or one side of a proposal version, as the side of the selection says. */
export type CodeAnchor =
  | { kind: "revision"; revision: string }
  | { kind: "version"; proposal: string; version: number };

export const REQUEST_NAME_NOTE =
  "Tell fold your name first, then make a request about these lines.";

/** The body of `POST /requests/from-code` for the selection: a revision takes the lines as they are; a version takes them from the selection's side. */
export function codeRequestBody(
  anchor: CodeAnchor,
  selection: DiffSelection,
  body: string,
): CodeRequestBody {
  const lines = {
    body,
    path: selection.path,
    start_line: selection.start_line,
    end_line: selection.end_line,
  };
  return anchor.kind === "revision"
    ? { ...lines, revision: anchor.revision }
    : {
        ...lines,
        proposal: anchor.proposal,
        version: anchor.version,
        side: selection.side,
      };
}

/** The lines in words: `lines 2–3 of web/src/board.ts in r17`, or `line 4 of web/src/board.ts in P-3 v2, new side`. */
export function codeText(code: RequestCode): string {
  const lines = `${linesText(rangeOf(code, "new"))} of ${code.path}`;
  if (code.revision !== null) return `${lines} in ${code.revision}`;
  const side = code.side === null ? "" : `, ${code.side} side`;
  return `${lines} in ${code.proposal ?? ""} v${String(code.version ?? 0)}${side}`;
}

/** The screen that shows the lines: the file in the repository at the revision, opened at the first line, or the file on the Changes tab of the version. */
export function codeHref(slug: string, code: RequestCode): string {
  if (code.revision !== null) {
    return withQuery(projectPath(slug, "repository"), {
      revision: code.revision,
      path: code.path,
      line: String(code.start_line),
    });
  }
  return detailHref(slug, code.proposal ?? "", {
    tab: "changes",
    ...(code.version === null ? {} : { version: code.version }),
    path: code.path,
  });
}

/** The selection's composer head: `Make a request about lines 2–3 · r17`, or with the side and version of a proposal. */
export function makeRequestTitle(
  anchor: CodeAnchor,
  selection: DiffSelection,
): string {
  const where =
    anchor.kind === "revision"
      ? anchor.revision
      : `${selection.side} side · v${String(anchor.version)}`;
  return `Make a request about ${linesText(selection)} · ${where}`;
}

/** The anchor of a comparison side, when it is a revision of main; a workspace or a checkpoint changes, so lines there take no request. */
export function revisionAnchor(side: {
  kind: string;
  id: string;
}): CodeAnchor | null {
  return side.kind === "revision"
    ? { kind: "revision", revision: side.id }
    : null;
}

/** The requests made on lines of the revision, drawn on `side` of a comparison: the new side of a file view or of a comparison's newer revision, the old side of its older one. */
export function revisionRequestMarks(
  requests: readonly RequestSummary[],
  revision: string,
  side: "old" | "new",
): DiffThread[] {
  return requests.flatMap((request) =>
    request.code?.revision === revision
      ? [{ id: request.id, range: rangeOf(request.code, side) }]
      : [],
  );
}

/** The requests made on lines of the shown version, on the sides whose lines are the version's own: a change since an earlier version shows only its new side. */
export function requestMarks(
  requests: readonly RequestSummary[],
  anchor: CodeAnchor,
  sides: readonly ("old" | "new")[] = ["old", "new"],
): DiffThread[] {
  if (anchor.kind === "revision") {
    return revisionRequestMarks(requests, anchor.revision, "new");
  }
  return requests.flatMap((request) => {
    const code = request.code;
    if (code === null) return [];
    const side = code.side ?? "new";
    return code.proposal === anchor.proposal &&
      code.version === anchor.version &&
      sides.includes(side)
      ? [{ id: request.id, range: rangeOf(code, side) }]
      : [];
  });
}

/** A file of a comparison: a renamed file's old side holds its lines at `previous_path`. */
export interface ComparedPaths {
  path: string;
  previous_path: string | null;
}

/** The path the tree of the selection's side holds the file at: the old side of a renamed file is at its previous path. */
export function sidePath(file: ComparedPaths, side: "old" | "new"): string {
  return side === "old" && file.previous_path !== null
    ? file.previous_path
    : file.path;
}

/** The selection as the tree of its side names it, so a request on the old side of a renamed file reads the file where that revision has it. */
export function sideSelection(
  selection: DiffSelection,
  file: ComparedPaths | null,
): DiffSelection {
  return file === null || file.path !== selection.path
    ? selection
    : { ...selection, path: sidePath(file, selection.side) };
}

/** The requests made on either revision of a comparison of two revisions; a workspace or a checkpoint side marks none. With the file on screen, a request on the older revision at its previous path is drawn on the file, and one at the new path of a renamed file is not, because that path held another file there. */
export function compareMarks(
  requests: readonly RequestSummary[],
  comparison: {
    from: { kind: string; id: string };
    to: { kind: string; id: string };
  },
  file: ComparedPaths | null = null,
): DiffThread[] {
  const from = revisionAnchor(comparison.from);
  const to = revisionAnchor(comparison.to);
  const older =
    from?.kind === "revision"
      ? revisionRequestMarks(requests, from.revision, "old")
      : [];
  return [
    ...(file === null || file.previous_path === null
      ? older
      : older.flatMap((mark) =>
          mark.range.path === file.previous_path
            ? [{ ...mark, range: { ...mark.range, path: file.path } }]
            : mark.range.path === file.path
              ? []
              : [mark],
        )),
    ...(to?.kind === "revision"
      ? revisionRequestMarks(requests, to.revision, "new")
      : []),
  ];
}

function rangeOf(code: RequestCode, side: "old" | "new"): DiffSelection {
  return {
    path: code.path,
    side,
    start_line: code.start_line,
    end_line: code.end_line,
  };
}

/** The marker's name and tooltip: `Request RQ-4 about lines 2–3, open`, with the title once an orchestrator named it. */
export function requestMarkLabel(request: RequestSummary): string {
  const range =
    request.code === null
      ? ""
      : ` about ${linesText(rangeOf(request.code, "new"))}`;
  const title = request.title === null ? "" : `: ${request.title}`;
  return `Request ${request.id}${range}${title}, ${STATE_LABELS[request.state].toLowerCase()}`;
}

/** An open request's mark stands out; an approved or closed one is quiet. */
export function requestMarkActive(request: RequestSummary): boolean {
  return !isFinal(request.state);
}
