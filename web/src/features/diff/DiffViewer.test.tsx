// @vitest-environment jsdom
import {
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ComparedFile, FileDiff, FileWarning } from "@/api/diff";
import { add, comparison, ctx, del, fileDiff, hunk } from "@/test/diff";
import { DiffViewer, type FileDiffView } from "./DiffViewer";
import type { FileTextView } from "./gaps";
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
  fileText?: FileTextView;
  warnings?: FileWarning[];
  threads?: DiffThread[];
  rowLimit?: number;
  onSelect?: (selection: DiffSelection | null) => void;
  initialPath?: string;
}

function Harness({
  fileDiff = { status: "ready", diff: DIFF },
  fileText,
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
        fileText={fileText}
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

  it("colours the code of a file whose language fold knows, over the added and removed fills", async () => {
    const diff = fileDiff(A, [
      hunk(1, 1, [del("let total = 1;"), add("const total = 2;")]),
    ]);
    render(<Harness fileDiff={{ status: "ready", diff }} />);
    const added = cell(/^Added line, new 1: const total = 2;$/);
    const removed = cell(/^Removed line, old 1: let total = 1;$/);
    await waitFor(() => {
      expect(added.querySelector("code span")).not.toBeNull();
    });
    const colour = (row: HTMLElement, text: string) =>
      [...row.querySelectorAll<HTMLElement>("code span")].find(
        (span) => span.textContent === text,
      )?.style.color;
    expect(colour(added, "const")).toBe("var(--syntax-keyword)");
    expect(colour(removed, "let")).toBe("var(--syntax-keyword)");
    expect(added.querySelector("code")?.textContent).toBe("const total = 2;");
    expect(added.className).toContain("bg-diff-add");
    expect(removed.className).toContain("bg-diff-del");
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

  describe("the unchanged lines around the hunks", () => {
    // 80 old lines; new line 30 is changed and a line is added after line 59, so the new file has 81 lines and old = new - 1 below it.
    const OLD = Array.from({ length: 80 }, (_, i) => `l${String(i + 1)}`);
    const NEW = [
      ...OLD.slice(0, 29),
      "L30",
      ...OLD.slice(30, 59),
      "added",
      ...OLD.slice(59),
    ];
    const LONG: FileDiff = fileDiff(A, [
      hunk(27, 27, [
        ...OLD.slice(26, 29).map(ctx),
        del("l30"),
        add("L30"),
        ...OLD.slice(30, 33).map(ctx),
      ]),
      hunk(57, 57, [
        ...OLD.slice(56, 59).map(ctx),
        add("added"),
        ...OLD.slice(59, 62).map(ctx),
      ]),
    ]);
    const FILE = {
      hash: A.new_hash ?? "",
      binary: false,
      too_large: false,
      text: NEW.map((l) => `${l}\n`).join(""),
    };
    const TEXT: FileTextView = { status: "ready", file: FILE };
    const long = { status: "ready", diff: LONG } as const;
    const button = (name: string | RegExp) =>
      screen.getByRole("button", { name });

    it("names the hidden lines of each gap with the controls that show them", () => {
      render(<Harness fileDiff={long} fileText={TEXT} />);
      expect(button("Show 26 unchanged lines")).toBeTruthy();
      expect(button("Show lines 7–26")).toBeTruthy();
      expect(
        screen.queryByRole("button", { name: "Show lines 1–20" }),
      ).toBeNull();
      expect(button("Show 23 unchanged lines")).toBeTruthy();
      expect(button("Show lines 34–53")).toBeTruthy();
      expect(button("Show lines 37–56")).toBeTruthy();
      expect(button("Show 18 unchanged lines")).toBeTruthy();
      expect(screen.getByText("@@ −27,7 +27,7 @@")).toBeTruthy();
      expect(screen.getByText("@@ −57,6 +57,7 @@")).toBeTruthy();
    });

    it("shows a step of lines with both numbers and moves focus to the first of them", async () => {
      const user = userEvent.setup();
      render(<Harness fileDiff={long} fileText={TEXT} />);
      await user.click(button("Show lines 34–53"));
      const first = cell(/^Unchanged line, old 34, new 34: l34$/);
      expect(document.activeElement).toBe(first);
      expect(cell(/^Unchanged line, old 53, new 53: l53$/)).toBeTruthy();
      expect(button("Show 3 unchanged lines")).toBeTruthy();
      expect(screen.queryByText("@@ −57,6 +57,7 @@")).toBeNull();
      await user.click(button("Show 18 unchanged lines"));
      expect(cell(/^Unchanged line, old 80, new 81: l80$/)).toBeTruthy();
      expect(document.activeElement).toBe(
        cell(/^Unchanged line, old 63, new 64: l63$/),
      );
    });

    it("shows the whole file and only the changes again", async () => {
      const user = userEvent.setup();
      render(<Harness fileDiff={long} fileText={TEXT} />);
      await user.click(button("Show lines 7–26"));
      await user.click(button("Show whole file"));
      expect(
        screen.queryByRole("button", { name: /unchanged lines/ }),
      ).toBeNull();
      expect(screen.queryByText(/^@@/)).toBeNull();
      expect(
        screen.getAllByRole("gridcell", { name: /, new \d+:/ }),
      ).toHaveLength(81);
      await user.click(button("Show changes only"));
      expect(button("Show 26 unchanged lines")).toBeTruthy();
      expect(screen.queryByRole("gridcell", { name: /new 7: l7$/ })).toBeNull();
    });

    it("starts another version of the file with its hunks only, and keeps the whole file shown", async () => {
      const user = userEvent.setup();
      const hash = "d".repeat(64);
      const next = {
        status: "ready",
        diff: { ...LONG, new_hash: hash },
      } as const;
      const nextText: FileTextView = {
        status: "ready",
        file: { ...FILE, hash },
      };
      const { rerender } = render(<Harness fileDiff={long} fileText={TEXT} />);
      await user.click(button("Show lines 34–53"));
      expect(cell(/^Unchanged line, old 34, new 34: l34$/)).toBeTruthy();
      rerender(<Harness fileDiff={next} fileText={nextText} />);
      expect(
        screen.queryByRole("gridcell", { name: /new 34: l34$/ }),
      ).toBeNull();
      expect(button("Show 23 unchanged lines")).toBeTruthy();
      await user.click(button("Show whole file"));
      rerender(<Harness fileDiff={long} fileText={TEXT} />);
      expect(button("Show changes only")).toBeTruthy();
      expect(
        screen.getAllByRole("gridcell", { name: /, new \d+:/ }),
      ).toHaveLength(81);
    });

    it("selects an expanded line on the new side, as any unchanged line", async () => {
      const user = userEvent.setup();
      render(<Harness fileDiff={long} fileText={TEXT} />);
      await user.click(button("Show 18 unchanged lines"));
      await user.click(cell(/old 69, new 70: l69$/));
      expect(selection()).toEqual({
        path: "src/a.ts",
        side: "new",
        start_line: 70,
        end_line: 70,
      });
      expect(screen.getByText("Composer new 70-70")).toBeTruthy();
    });

    it("keeps the lines around a thread in a gap on screen", () => {
      render(
        <Harness
          fileDiff={long}
          fileText={TEXT}
          threads={[
            {
              id: "t9",
              range: {
                path: "src/a.ts",
                side: "old",
                start_line: 69,
                end_line: 69,
              },
            },
          ]}
        />,
      );
      expect(screen.getByText("Thread t9")).toBeTruthy();
      expect(cell(/old 66, new 67: l66$/)).toBeTruthy();
      expect(cell(/old 72, new 73: l72$/)).toBeTruthy();
      expect(screen.queryByRole("gridcell", { name: /new 74:/ })).toBeNull();
      expect(button("Show 3 unchanged lines")).toBeTruthy();
      expect(button("Show 8 unchanged lines")).toBeTruthy();
    });

    it("waits with the controls while the file loads, and offers Retry when it fails", async () => {
      const user = userEvent.setup();
      const retry = vi.fn();
      const { rerender } = render(
        <Harness fileDiff={long} fileText={{ status: "pending" }} />,
      );
      const loading = screen.getAllByRole("button", {
        name: "Loading unchanged lines…",
      });
      expect(loading).toHaveLength(2);
      expect(loading.every((b) => b.hasAttribute("disabled"))).toBe(true);
      expect(button("Loading the file…").hasAttribute("disabled")).toBe(true);
      expect(screen.getByText("@@ −27,7 +27,7 @@")).toBeTruthy();
      rerender(
        <Harness fileDiff={long} fileText={{ status: "error", retry }} />,
      );
      expect(
        screen.getAllByText("The unchanged lines could not load."),
      ).toHaveLength(2);
      expect(screen.queryByRole("button", { name: /whole file/ })).toBeNull();
      await user.click(
        screen.getAllByRole("button", { name: "Retry" })[0] as HTMLElement,
      );
      expect(retry).toHaveBeenCalledTimes(1);
    });

    it("shows only the hunks when the read file is not the one they were built from", () => {
      const other: FileTextView = {
        status: "ready",
        file: { ...FILE, hash: "c".repeat(64) },
      };
      render(<Harness fileDiff={long} fileText={other} />);
      expect(
        screen.queryByRole("button", { name: /unchanged lines/ }),
      ).toBeNull();
      expect(screen.queryByRole("button", { name: /whole file/ })).toBeNull();
      expect(screen.getByText("@@ −27,7 +27,7 @@")).toBeTruthy();
    });

    it("offers no whole file when the hunks hold every line", () => {
      const diff = fileDiff(A, [hunk(1, 1, [del("a"), add("b")])]);
      render(
        <Harness
          fileDiff={{ status: "ready", diff }}
          fileText={{
            status: "ready",
            file: {
              hash: A.new_hash ?? "",
              binary: false,
              too_large: false,
              text: "b\n",
            },
          }}
        />,
      );
      expect(screen.queryByRole("button", { name: /whole file/ })).toBeNull();
      expect(screen.getByText("@@ −1,1 +1,1 @@")).toBeTruthy();
    });
  });
});
