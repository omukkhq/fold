import { ChevronDown, ChevronRight, Folder } from "lucide-react";
import { useId } from "react";
import { controlClasses } from "@/components/form/Field";
import { changeBadge, lineCounts } from "@/features/diff/present";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { treeRows, type ChangeNode, type TreeRow } from "./folderTree";

const ROW =
  "flex w-full min-w-0 items-center gap-8 rounded-s py-5 pr-8 text-left hover:bg-sunken";

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
        <span className="shrink-0 font-mono text-caption text-primary-text">
          {counts.added}
        </span>
      ) : null}
      {counts.removed ? (
        <span className="shrink-0 font-mono text-caption text-danger">
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
            size={13}
            className="shrink-0 text-text-faint"
          />
          <Folder
            aria-hidden="true"
            size={13}
            className="shrink-0 text-primary-muted"
          />
          <span className="min-w-0 flex-1 truncate font-mono text-label text-text-muted">
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
          selected && "bg-primary-soft hover:bg-primary-soft",
          focusRing,
        )}
        onClick={() => {
          onSelect(node.path);
        }}
      >
        <span aria-hidden="true" className="size-13 shrink-0" />
        <span
          className={cn(
            "flex size-16 shrink-0 items-center justify-center rounded-s font-mono text-caption font-bold",
            badge.classes,
          )}
        >
          <span aria-hidden="true">{badge.letter}</span>
          <span className="sr-only">{badge.label}</span>
        </span>
        <span
          className={cn(
            "min-w-0 flex-1 truncate font-mono text-label",
            viewed ? "text-text-faint" : "text-text",
          )}
        >
          {node.name}
          {viewed ? <span className="sr-only"> (viewed)</span> : null}
        </span>
        {node.file.binary ? (
          <span className="font-mono text-caption text-text-muted">bin</span>
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
    "rounded-s text-meta font-medium text-primary-text hover:underline",
    focusRing,
  );
  return (
    <nav
      aria-label="Changed files"
      className="flex min-w-0 flex-col gap-8 lg:sticky lg:top-0 lg:w-review-panel lg:shrink-0"
    >
      <label
        htmlFor={id}
        className="text-overline leading-snug font-semibold text-text-muted uppercase"
      >
        Files
      </label>
      <input
        id={id}
        type="search"
        value={filter}
        placeholder="Filter files"
        onChange={(event) => {
          onFilter(event.target.value);
        }}
        className={cn(controlClasses, focusRing)}
      />
      <div className="flex flex-wrap items-center gap-x-8 gap-y-2 px-4">
        <button type="button" className={action} onClick={onExpandAll}>
          Expand all
        </button>
        <span aria-hidden="true" className="text-text-faint">
          ·
        </span>
        <button type="button" className={action} onClick={onCollapseAll}>
          Collapse all
        </button>
        <span className="ml-auto text-meta text-text-faint">{viewedText}</span>
      </div>
      {rows.length === 0 ? (
        <p className="px-4 text-code text-text-muted">No file matches.</p>
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
