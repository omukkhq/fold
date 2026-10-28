import {
  ArrowDownToLine,
  ArrowUpToLine,
  MessageSquare,
  RotateCw,
  UnfoldVertical,
} from "lucide-react";
import {
  Fragment,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { Button, IconButton } from "@/components/button/Button";
import { SyntaxTokens } from "@/components/code/SyntaxText";
import { Tooltip } from "@/components/overlay/Tooltip";
import { DiffLine } from "@/components/diff/DiffLine";
import { cn } from "@/lib/cn";
import { languageForPath } from "@/syntax/languages";
import { useHighlight } from "@/syntax/useHighlight";
import { EXPAND_STEP, type ExpandHow } from "./gaps";
import type { DiffRow, GapRow, LineRow } from "./layout";
import {
  GAP_ERROR,
  NO_NEWLINE,
  hiddenLinesText,
  lineLabel,
  showAllText,
  showLinesText,
  unchangedLinesText,
} from "./present";
import { diffSyntax, rowTokens } from "./syntax";
import {
  EMPTY_STATE,
  isLastSelected,
  isSelected,
  keyCommand,
  marksAt,
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
  /** Shows hidden lines of a gap and answers the new line that takes focus. */
  onGap?: (row: GapRow, how: ExpandHow) => number;
  /** Reads the file's text again after it failed to load. */
  onGapRetry?: () => void;
  threads?: readonly DiffThread[];
  renderThread?: (thread: DiffThread) => ReactNode;
  renderComposer?: (selection: DiffSelection) => ReactNode;
  /** Ranges that carry a mark at each of their lines besides the review threads, such as questions to the author. */
  marks?: readonly DiffThread[];
  /** `first` is true on the first line of the mark's range. */
  renderMark?: (mark: DiffThread, first: boolean) => ReactNode;
}

// A row spans the panel, so its focus ring sits inside the row instead of outside it.
const ROW_FOCUS =
  "cursor-pointer focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent";

/**
 * A block under a line, indented past the number and sign columns. A review thread is a quiet block with an accent edge on its left; the selection's menu and forms sit in the same indent without a block, so the lines around them stay one surface.
 */
function UnderLine({
  thread = false,
  children,
}: {
  thread?: boolean;
  children: ReactNode;
}) {
  return (
    <div role="row">
      <div role="gridcell" className="flex py-8 pr-16">
        <span aria-hidden="true" className="w-diff-number shrink-0" />
        <span aria-hidden="true" className="w-diff-number shrink-0" />
        <span aria-hidden="true" className="w-24 shrink-0" />
        <div
          className={cn(
            "min-w-0 flex-1",
            thread &&
              "max-w-measure rounded-r-m border-l-3 border-accent bg-sunken/55 px-14 py-10",
          )}
        >
          {children}
        </div>
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
  onGap,
  onGapRetry,
  threads = [],
  renderThread,
  renderComposer,
  marks = [],
  renderMark,
}: DiffRowsProps) {
  const [state, setState] = useState(EMPTY_STATE);
  const stateRef = useRef(state);
  const cells = useRef(new Map<string, HTMLElement>());
  const points = pointsOf(rows);
  const stop = tabStop(state.cursor, points);
  const syntax = diffSyntax(rows, languageForPath(path));
  const tokens = useHighlight(syntax.request);
  // A control the reader used goes away with the lines it showed, so focus moves to the first of them.
  const focusAfter = useRef<string | null>(null);
  useEffect(() => {
    const key = focusAfter.current;
    if (key === null) return;
    focusAfter.current = null;
    cells.current.get(key)?.focus();
  });

  function expand(row: GapRow, how: ExpandHow) {
    if (!onGap) return;
    const line = onGap(row, how);
    focusAfter.current = pointKey({ side: "new", line });
  }

  function gap(row: GapRow) {
    const pending = row.status === "pending";
    const step = Math.min(EXPAND_STEP, row.hidden);
    const up = showLinesText(row.last - step + 1, row.last);
    const down = showLinesText(row.first, row.first + step - 1);
    return (
      <div role="row" key={row.key}>
        <div
          role="gridcell"
          aria-label={unchangedLinesText(row.hidden)}
          className="flex min-h-32 flex-wrap items-center gap-x-4 gap-y-2 bg-diff-hunk py-3 pr-10 pl-6 text-13 text-text-2"
        >
          {row.status === "error" ? (
            <>
              <span className="pl-8">{GAP_ERROR}</span>
              <Button
                variant="ghost"
                size="sm"
                icon={<RotateCw aria-hidden="true" size={14} />}
                onClick={onGapRetry}
              >
                Retry
              </Button>
            </>
          ) : (
            <>
              {row.up ? (
                <Tooltip content={up}>
                  <IconButton
                    variant="ghost"
                    size="sm"
                    aria-label={up}
                    pending={pending}
                    pendingLabel="Loading lines…"
                    onClick={() => {
                      expand(row, "up");
                    }}
                  >
                    <ArrowUpToLine aria-hidden="true" size={14} />
                  </IconButton>
                </Tooltip>
              ) : null}
              {row.down ? (
                <Tooltip content={down}>
                  <IconButton
                    variant="ghost"
                    size="sm"
                    aria-label={down}
                    pending={pending}
                    pendingLabel="Loading lines…"
                    onClick={() => {
                      expand(row, "down");
                    }}
                  >
                    <ArrowDownToLine aria-hidden="true" size={14} />
                  </IconButton>
                </Tooltip>
              ) : null}
              <span aria-hidden="true" className="ml-6">
                {unchangedLinesText(row.hidden)}
              </span>
              <Button
                variant="ghost"
                size="sm"
                aria-label={pending ? undefined : showAllText(row.hidden)}
                className="text-accent hover:bg-surface hover:text-accent"
                icon={<UnfoldVertical aria-hidden="true" size={14} />}
                pending={pending}
                pendingLabel="Loading unchanged lines…"
                onClick={() => {
                  expand(row, "all");
                }}
              >
                Show all
              </Button>
            </>
          )}
          {row.header === null ? null : (
            <code className="min-w-0 font-mono text-13 leading-tight wrap-anywhere text-text-2">
              {row.header}
            </code>
          )}
        </div>
      </div>
    );
  }

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
    const marked = renderMark ? marksAt(marks, path, drawn) : [];
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
              covering.length > 0 || marked.length > 0 ? (
                <span className="flex items-center gap-4">
                  {covering.length > 0 ? (
                    <MessageSquare
                      aria-hidden="true"
                      size={13}
                      className="text-diff-comment-marker"
                    />
                  ) : null}
                  {marked.map(({ mark, first }) => (
                    <Fragment key={mark.id}>
                      {renderMark?.(mark, first)}
                    </Fragment>
                  ))}
                </span>
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
              <span aria-hidden="true" className="ml-8 text-13 text-text-2">
                {NO_NEWLINE}
              </span>
            ) : null}
          </DiffLine>
        </div>
        {renderThread
          ? ending.map((thread) => (
              <UnderLine key={thread.id} thread>
                {renderThread(thread)}
              </UnderLine>
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
        if (row.type === "gap") return gap(row);
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
                  "flex min-h-32 flex-1 items-center gap-6 px-14 py-3 text-left text-13 font-semibold text-accent hover:text-text",
                  ROW_FOCUS,
                )}
              >
                <UnfoldVertical aria-hidden="true" size={14} />
                {hiddenLinesText(row.hidden)}
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
}
