import { MessageSquare, UnfoldVertical } from "lucide-react";
import {
  Fragment,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { SyntaxTokens } from "@/components/code/SyntaxText";
import { DiffLine } from "@/components/diff/DiffLine";
import { cn } from "@/lib/cn";
import { languageForPath } from "@/syntax/languages";
import { useHighlight } from "@/syntax/useHighlight";
import type { DiffRow, LineRow } from "./layout";
import { NO_NEWLINE, hiddenLinesText, lineLabel } from "./present";
import { diffSyntax, rowTokens } from "./syntax";
import {
  EMPTY_STATE,
  isLastSelected,
  isSelected,
  keyCommand,
  pointKey,
  pointOf,
  pointsOf,
  reduceSelection,
  sameSelection,
  samePoint,
  tabStop,
  threadPointsOf,
  threadsCovering,
  threadsEndingAt,
  type DiffSelection,
  type DiffThread,
  type SelectionAction,
} from "./selection";

export interface DiffRowsProps {
  path: string;
  rows: readonly DiffRow[];
  selection: DiffSelection | null;
  onSelect: (selection: DiffSelection | null) => void;
  onExpand: (key: string) => void;
  threads?: readonly DiffThread[];
  renderThread?: (thread: DiffThread) => ReactNode;
  renderComposer?: (selection: DiffSelection) => ReactNode;
}

// A row spans the panel, so its focus ring sits inside the row instead of outside it.
const ROW_FOCUS =
  "cursor-pointer outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus-ring";

/** A block under a line, indented past the number and sign columns. */
function UnderLine({ children }: { children: ReactNode }) {
  return (
    <div role="row">
      <div role="gridcell" className="flex border-y border-border-subtle bg-bg">
        <span aria-hidden="true" className="w-diff-number shrink-0" />
        <span aria-hidden="true" className="w-diff-number shrink-0" />
        <span aria-hidden="true" className="w-24 shrink-0" />
        <div className="min-w-0 flex-1 pt-10 pr-14 pb-12">{children}</div>
      </div>
    </div>
  );
}

export function DiffRows({
  path,
  rows,
  selection,
  onSelect,
  onExpand,
  threads = [],
  renderThread,
  renderComposer,
}: DiffRowsProps) {
  const [state, setState] = useState(EMPTY_STATE);
  const stateRef = useRef(state);
  const cells = useRef(new Map<string, HTMLElement>());
  const points = pointsOf(rows);
  const stop = tabStop(state.cursor, points);
  const syntax = diffSyntax(rows, languageForPath(path));
  const tokens = useHighlight(syntax.request);

  function dispatch(action: SelectionAction) {
    const result = reduceSelection(stateRef.current, action, {
      path,
      points,
      selection,
    });
    const moved = !samePoint(result.state.cursor, stateRef.current.cursor);
    stateRef.current = result.state;
    setState(result.state);
    if (!sameSelection(result.selection, selection)) {
      onSelect(result.selection);
    }
    const cursor = result.state.cursor;
    if (moved && action.type === "key" && cursor) {
      cells.current.get(pointKey(cursor))?.focus();
    }
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (!(event.target instanceof HTMLElement)) return;
    if (!event.target.hasAttribute("data-line")) return;
    const command = keyCommand(event.key, event.shiftKey);
    if (command === null) return;
    event.preventDefault();
    dispatch({ type: "key", command });
  }

  function line(row: LineRow) {
    const point = pointOf(row);
    if (!point) return null;
    const key = pointKey(point);
    const drawn = threadPointsOf(row);
    const covering = drawn.flatMap((p) => threadsCovering(threads, path, p));
    const ending = drawn.flatMap((p) => threadsEndingAt(threads, path, p));
    const selected = isSelected(selection, path, point);
    const lineTokens = rowTokens(syntax, tokens, row.key);
    return (
      <Fragment key={row.key}>
        <div role="row">
          <DiffLine
            role="gridcell"
            data-line={key}
            aria-selected={selected}
            aria-label={lineLabel(row)}
            tabIndex={samePoint(point, stop) ? 0 : -1}
            ref={(el) => {
              if (el) cells.current.set(key, el);
              else cells.current.delete(key);
            }}
            kind={row.kind}
            oldNo={row.oldNo ?? undefined}
            newNo={row.newNo ?? undefined}
            selected={selected}
            className={ROW_FOCUS}
            marker={
              covering.length > 0 ? (
                <MessageSquare
                  aria-hidden="true"
                  size={13}
                  className="text-diff-comment-marker"
                />
              ) : undefined
            }
            onMouseDown={(event) => {
              // Shift-click extends the selection; without this the browser also selects text.
              if (event.shiftKey) event.preventDefault();
            }}
            onFocus={() => {
              dispatch({ type: "focus", point });
            }}
            onClick={(event) => {
              dispatch({ type: "click", point, shift: event.shiftKey });
            }}
          >
            {lineTokens ? <SyntaxTokens tokens={lineTokens} /> : row.text}
            {row.noNewline ? (
              <span
                aria-hidden="true"
                className="ml-8 text-meta text-text-muted"
              >
                {NO_NEWLINE}
              </span>
            ) : null}
          </DiffLine>
        </div>
        {renderThread
          ? ending.map((thread) => (
              <UnderLine key={thread.id}>{renderThread(thread)}</UnderLine>
            ))
          : null}
        {renderComposer &&
        selection &&
        isLastSelected(selection, path, point) ? (
          <UnderLine>{renderComposer(selection)}</UnderLine>
        ) : null}
      </Fragment>
    );
  }

  return (
    <div
      role="grid"
      aria-label={`Diff of ${path}`}
      aria-multiselectable="true"
      onKeyDown={onKeyDown}
    >
      {rows.map((row) => {
        if (row.type === "line") return line(row);
        if (row.type === "hunk") {
          return (
            <div role="row" key={row.key}>
              <DiffLine role="gridcell" kind="hunk">
                {row.header}
              </DiffLine>
            </div>
          );
        }
        return (
          <div role="row" key={row.key}>
            <div role="gridcell" className="flex bg-diff-hunk">
              <button
                type="button"
                onClick={() => {
                  onExpand(row.key);
                }}
                className={cn(
                  "flex flex-1 items-center gap-6 px-14 py-4 text-left font-mono text-label text-text-muted hover:text-text",
                  ROW_FOCUS,
                )}
              >
                <UnfoldVertical aria-hidden="true" size={13} />
                {hiddenLinesText(row.hidden)}
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
}
