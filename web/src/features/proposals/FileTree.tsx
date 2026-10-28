import { ChevronDown, ChevronRight, Folder, Search } from "lucide-react";
import { useId } from "react";
import { controlClasses } from "@/components/form/Field";
import { changeBadge, lineCounts } from "@/features/diff/present";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { treeRows, type ChangeNode, type TreeRow } from "./folderTree";

const ROW =
  "flex w-full min-w-0 items-center gap-8 rounded-s py-5 pr-8 text-left hover:bg-surface/60";

/** Each level of the tree steps in by the same space. */
function indent(depth: number): { paddingInlineStart: string } {
  return {
    paddingInlineStart: `calc(var(--space-8) + var(--space-14) * ${String(depth)})`,
  };
}

function Counts({ added, removed }: { added: number; removed: number }) {
  const counts = lineCounts({ added_lines: added, removed_lines: removed });
  return (
    <>
      {counts.added ? (
        <span className="shrink-0 font-mono text-13 text-ok">
          {counts.added}
        </span>
      ) : null}
      {counts.removed ? (
        <span className="shrink-0 font-mono text-13 text-bad">
          {counts.removed}
        </span>
      ) : null}
    </>
  );
}

function Row({
  row,
  selected,
  viewed,
  onToggle,
  onSelect,
}: {
  row: TreeRow;
  selected: boolean;
  viewed: boolean;
  onToggle: (path: string) => void;
  onSelect: (path: string) => void;
}) {
  const node = row.node;
  if (node.kind === "folder") {
    const Chevron = row.open ? ChevronDown : ChevronRight;
    return (
      <li>
        <button
          type="button"
          aria-expanded={row.open}
          style={indent(row.depth)}
          className={cn(ROW, focusRing)}
          onClick={() => {
            onToggle(node.path);
          }}
        >
          <Chevron
            aria-hidden="true"
            size={14}
            className="shrink-0 text-text-2"
          />
          <Folder
            aria-hidden="true"
            size={14}
            className="shrink-0 text-text-2"
          />
          <span className="min-w-0 flex-1 truncate font-mono text-13 text-text-2">
            {node.name}/
          </span>
          <Counts added={node.totals.added} removed={node.totals.removed} />
        </button>
      </li>
    );
  }
  const badge = changeBadge(node.file.change);
  return (
    <li>
      <button
        type="button"
        aria-current={selected ? "true" : undefined}
        data-viewed={viewed || undefined}
        title={node.path}
        style={indent(row.depth)}
        className={cn(
          ROW,
          selected && "bg-surface hover:bg-surface",
          focusRing,
        )}
        onClick={() => {
          onSelect(node.path);
        }}
      >
        <span aria-hidden="true" className="size-14 shrink-0" />
        <span
          className={cn(
            "w-14 shrink-0 text-center font-title text-13 font-bold",
            badge.classes,
          )}
        >
          <span aria-hidden="true">{badge.letter}</span>
          <span className="sr-only">{badge.label}</span>
        </span>
        <span
          className={cn(
            "min-w-0 flex-1 truncate font-mono text-13",
            viewed ? "text-text-2" : "text-text",
          )}
        >
          {node.name}
          {viewed ? <span className="sr-only"> (viewed)</span> : null}
        </span>
        {node.file.binary ? (
          <span className="font-mono text-13 text-text-2">bin</span>
        ) : null}
        <Counts
          added={node.file.added_lines}
          removed={node.file.removed_lines}
        />
      </button>
    </li>
  );
}

export interface FileTreeProps {
  nodes: readonly ChangeNode[];
  selectedPath: string | null;
  viewed: ReadonlySet<string>;
  /** `2 of 33 viewed`. */
  viewedText: string;
  closed: ReadonlySet<string>;
  filter: string;
  onFilter: (filter: string) => void;
  onToggle: (path: string) => void;
  onExpandAll: () => void;
  onCollapseAll: () => void;
  onSelect: (path: string) => void;
}

/** The changed files as folders that open and close, with a filter; a folder shows the lines added and removed below it. */
export function FileTree({
  nodes,
  selectedPath,
  viewed,
  viewedText,
  closed,
  filter,
  onFilter,
  onToggle,
  onExpandAll,
  onCollapseAll,
  onSelect,
}: FileTreeProps) {
  const id = useId();
  const rows = treeRows(nodes, closed, filter);
  const action = cn(
    "rounded-s text-13 font-semibold text-accent hover:underline",
    focusRing,
  );
  return (
    <nav
      aria-label="Changed files"
      className="flex min-w-0 flex-col gap-10 lg:sticky lg:w-review-panel lg:shrink-0"
      // Held in view under the page's bar while the diff scrolls; the proposal page measures its bar, and a page without one holds the tree at the top.
      style={{ top: "calc(var(--decision-bar-height, 0px) + var(--space-14))" }}
    >
      <div className="flex items-baseline justify-between gap-8">
        <label
          htmlFor={id}
          className="font-title text-16 leading-tight font-semibold text-text"
        >
          Files
        </label>
        <span className="text-13 text-text-2">{viewedText}</span>
      </div>
      <div className="relative">
        <Search
          aria-hidden="true"
          size={14}
          className="pointer-events-none absolute top-1/2 left-12 -translate-y-1/2 text-text-2"
        />
        <input
          id={id}
          type="search"
          value={filter}
          placeholder="Filter files"
          onChange={(event) => {
            onFilter(event.target.value);
          }}
          className={cn(controlClasses, "bg-surface pl-32 text-14", focusRing)}
        />
      </div>
      <div className="flex flex-wrap items-center gap-x-12 gap-y-2 px-4">
        <button type="button" className={action} onClick={onExpandAll}>
          Expand all
        </button>
        <button type="button" className={action} onClick={onCollapseAll}>
          Collapse all
        </button>
      </div>
      {rows.length === 0 ? (
        <p className="px-4 text-13 text-text-2">No file matches.</p>
      ) : (
        <ul className="flex flex-col">
          {rows.map((row) => (
            <Row
              key={`${row.node.kind}:${row.node.path}`}
              row={row}
              selected={row.node.path === selectedPath}
              viewed={viewed.has(row.node.path)}
              onToggle={onToggle}
              onSelect={onSelect}
            />
          ))}
        </ul>
      )}
    </nav>
  );
}
