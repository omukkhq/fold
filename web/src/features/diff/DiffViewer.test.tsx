// @vitest-environment jsdom
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ComparedFile, FileDiff, FileWarning } from "@/api/diff";
import { add, comparison, ctx, del, fileDiff, hunk } from "@/test/diff";
import { DiffViewer, type FileDiffView } from "./DiffViewer";
import type { DiffSelection, DiffThread } from "./selection";

afterEach(cleanup);

function file(path: string, extra: Partial<ComparedFile> = {}): ComparedFile {
  return {
    path,
    previous_path: null,
    change: "modified",
    added_lines: 2,
    removed_lines: 1,
    binary: false,
    old_hash: "a".repeat(64),
    new_hash: "b".repeat(64),
    ...extra,
  };
}

const A = file("src/a.ts");

const COMPARISON = comparison([
  A,
  file("img/logo.png", { binary: true, added_lines: 0, removed_lines: 0 }),
]);

// Context 1/1, removed 2, added 2 and 3, context 3/4.
const DIFF: FileDiff = fileDiff(A, [
  hunk(1, 1, [ctx("one"), del("two"), add("two!"), add("extra"), ctx("three")]),
]);

interface HarnessProps {
  fileDiff?: FileDiffView;
  warnings?: FileWarning[];
  threads?: DiffThread[];
  rowLimit?: number;
  onSelect?: (selection: DiffSelection | null) => void;
  initialPath?: string;
}

function Harness({
  fileDiff = { status: "ready", diff: DIFF },
  warnings = [],
  threads,
  rowLimit,
  onSelect,
  initialPath = "src/a.ts",
}: HarnessProps) {
  const [selection, setSelection] = useState<DiffSelection | null>(null);
  const [path, setPath] = useState(initialPath);
  return (
    <>
      <output data-testid="selection">{JSON.stringify(selection)}</output>
      <DiffViewer
        comparison={{ ...COMPARISON, warnings }}
        selectedPath={path}
        onSelectFile={setPath}
        fileDiff={fileDiff}
        selection={selection}
        onSelect={(next) => {
          onSelect?.(next);
          setSelection(next);
        }}
        threads={threads}
        renderThread={(thread) => <p>Thread {thread.id}</p>}
        renderComposer={(sel) => (
          <p>
            Composer {sel.side} {sel.start_line}-{sel.end_line}
          </p>
        )}
        rowLimit={rowLimit}
      />
    </>
  );
}

function selection(): DiffSelection | null {
  const text = screen.getByTestId("selection").textContent;
  return JSON.parse(text) as DiffSelection | null;
}

const cell = (name: RegExp) => screen.getByRole("gridcell", { name });

describe("DiffViewer", () => {
  it("lists the files and shows the selected file's rows", () => {
    render(<Harness />);
    const list = screen.getByRole("navigation", { name: "Changed files" });
    expect(within(list).getAllByRole("button")).toHaveLength(2);
    expect(screen.getByRole("heading", { name: "src/a.ts" })).toBeTruthy();
    expect(screen.getByText("@@ −1,3 +1,4 @@")).toBeTruthy();
    expect(cell(/^Added line, new 2: two!/)).toBeTruthy();
  });

  it("selects with the keyboard: Tab, arrows, Space, Shift and the arrows, Escape", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    // The file button is the last control before the grid, whose first line holds its Tab stop.
    await user.click(screen.getByRole("button", { name: /src\/a\.ts/ }));
    await user.tab();
    const grid = screen.getByRole("grid", { name: "Diff of src/a.ts" });
    expect(document.activeElement).toBe(cell(/new 1: one/));
    expect(
      within(grid)
        .getAllByRole("gridcell")
        .filter((c) => c.tabIndex === 0),
    ).toHaveLength(1);

    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(document.activeElement).toBe(cell(/new 2: two!/));
    await user.keyboard(" ");
    expect(selection()).toEqual({
      path: "src/a.ts",
      side: "new",
      start_line: 2,
      end_line: 2,
    });
    expect(cell(/new 2: two!/).getAttribute("aria-selected")).toBe("true");

    await user.keyboard("{Shift>}{ArrowDown}{ArrowDown}{/Shift}");
    expect(selection()).toEqual({
      path: "src/a.ts",
      side: "new",
      start_line: 2,
      end_line: 4,
    });
    expect(document.activeElement).toBe(cell(/new 4: three/));
    expect(screen.getByText("Composer new 2-4")).toBeTruthy();

    await user.keyboard("{Escape}");
    expect(selection()).toBeNull();
    expect(screen.queryByText(/Composer/)).toBeNull();
  });

  it("restarts the selection when Shift and an arrow cross to the other side", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(cell(/new 1: one/));
    await user.keyboard("{Shift>}{ArrowDown}{/Shift}");
    expect(selection()).toEqual({
      path: "src/a.ts",
      side: "old",
      start_line: 2,
      end_line: 2,
    });
  });

  it("selects a line with a click and a run with Shift-click", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(cell(/new 2: two!/));
    expect(selection()).toEqual({
      path: "src/a.ts",
      side: "new",
      start_line: 2,
      end_line: 2,
    });
    await user.keyboard("{Shift>}");
    await user.click(cell(/new 4: three/));
    await user.keyboard("{/Shift}");
    expect(selection()).toEqual({
      path: "src/a.ts",
      side: "new",
      start_line: 2,
      end_line: 4,
    });
    await user.keyboard("{Shift>}");
    await user.click(cell(/old 2: two$/));
    await user.keyboard("{/Shift}");
    expect(selection()).toEqual({
      path: "src/a.ts",
      side: "old",
      start_line: 2,
      end_line: 2,
    });
  });

  it("does not call onSelect when the selection stays the same", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(<Harness onSelect={onSelect} />);
    await user.click(cell(/new 2: two!/));
    await user.click(cell(/new 2: two!/));
    await user.keyboard("{ArrowDown}");
    expect(onSelect).toHaveBeenCalledTimes(1);
  });

  it("leaves other keys alone", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(<Harness onSelect={onSelect} />);
    await user.click(cell(/new 1: one/));
    await user.keyboard("a{Enter}");
    expect(onSelect).toHaveBeenCalledTimes(1);
  });

  it("marks lines that have a thread and shows the thread under its last line", () => {
    render(
      <Harness
        threads={[
          {
            id: "t1",
            range: {
              path: "src/a.ts",
              side: "new",
              start_line: 1,
              end_line: 2,
            },
          },
        ]}
      />,
    );
    expect(screen.getByText("Thread t1")).toBeTruthy();
    expect(cell(/new 1: one/).querySelector("svg")).not.toBeNull();
    expect(cell(/new 4: three/).querySelector("svg")).toBeNull();
  });

  it("shows a binary file's hashes and no rows", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByRole("button", { name: /img\/logo\.png/ }));
    expect(
      screen.getByText("Modified binary file, compared by hash."),
    ).toBeTruthy();
    expect(screen.getByText("a".repeat(64))).toBeTruthy();
    expect(screen.getByText("b".repeat(64))).toBeTruthy();
    expect(screen.queryByRole("grid")).toBeNull();
  });

  it("opens a collapsed run and shows more rows of a large file", async () => {
    const user = userEvent.setup();
    const long: FileDiff = fileDiff(A, [
      hunk(1, 1, [
        add("first"),
        ...Array.from({ length: 12 }, (_, i) => ctx(`same ${String(i + 1)}`)),
      ]),
    ]);
    render(<Harness fileDiff={{ status: "ready", diff: long }} rowLimit={6} />);
    await user.click(
      screen.getByRole("button", { name: "Show 9 unchanged lines" }),
    );
    expect(screen.queryByRole("button", { name: /unchanged/ })).toBeNull();
    expect(
      screen.getAllByRole("gridcell", { name: /^Unchanged/ }),
    ).toHaveLength(4);
    await user.click(screen.getByRole("button", { name: /Show more/ }));
    expect(
      screen.getAllByRole("gridcell", { name: /^Unchanged/ }),
    ).toHaveLength(10);
  });

  it("shows a file the daemon found too large by its hashes, not as an empty diff", () => {
    render(
      <Harness
        fileDiff={{
          status: "ready",
          diff: fileDiff(A, [], { too_large: true }),
        }}
      />,
    );
    expect(
      screen.getByText("Modified file over 1 MiB, compared by hash."),
    ).toBeTruthy();
    expect(screen.getByText("a".repeat(64))).toBeTruthy();
    expect(screen.queryByRole("grid")).toBeNull();
  });

  it("shows a file the hunks call binary by its hashes", () => {
    render(
      <Harness
        fileDiff={{ status: "ready", diff: fileDiff(A, [], { binary: true }) }}
      />,
    );
    expect(
      screen.getByText("Modified binary file, compared by hash."),
    ).toBeTruthy();
  });

  it("lists an unstable file and shows it as unstable instead of an empty diff", async () => {
    const user = userEvent.setup();
    render(
      <Harness warnings={[{ path: "logs/noisy.log", reason: "unstable" }]} />,
    );
    const list = screen.getByRole("navigation", { name: "Changed files" });
    const row = within(list).getByRole("button", { name: /noisy\.log/ });
    expect(row.textContent).toContain("unstable");
    await user.click(row);
    expect(
      screen.getByRole("heading", { name: "logs/noisy.log" }),
    ).toBeTruthy();
    expect(screen.getByText(/kept changing while fold read it/)).toBeTruthy();
    expect(screen.queryByRole("grid")).toBeNull();
  });

  it("shows a changed file that is also unstable as unstable", () => {
    render(<Harness warnings={[{ path: "src/a.ts", reason: "unstable" }]} />);
    const list = screen.getByRole("navigation", { name: "Changed files" });
    expect(within(list).getAllByRole("button")).toHaveLength(2);
    expect(screen.getByText(/kept changing while fold read it/)).toBeTruthy();
    expect(screen.queryByRole("grid")).toBeNull();
  });

  it("marks the last line of a file without a terminator", () => {
    const last = hunk(1, 1, [ctx("one"), add("end")]);
    const line = last.lines[1];
    if (line) {
      line.text = "end";
      line.no_newline = true;
    }
    render(
      <Harness fileDiff={{ status: "ready", diff: fileDiff(A, [last]) }} />,
    );
    expect(
      cell(/^Added line, new 2: end, no newline at end of file$/),
    ).toBeTruthy();
    expect(cell(/^Unchanged line, old 1, new 1: one$/)).toBeTruthy();
  });

  it("shows a skeleton while the file loads and an error with Retry when it fails", async () => {
    const user = userEvent.setup();
    const retry = vi.fn();
    const { rerender } = render(<Harness fileDiff={{ status: "pending" }} />);
    expect(screen.getByRole("status", { name: "Loading" })).toBeTruthy();
    rerender(<Harness fileDiff={{ status: "error", retry }} />);
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(retry).toHaveBeenCalledTimes(1);
  });
});
