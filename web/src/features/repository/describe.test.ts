import { describe, expect, it } from "vitest";
import type { FileView, Revision } from "../../api/repository";
import {
  describeRevision,
  fileState,
  formatSize,
  isOpenable,
  mainCopyText,
  symlinkLabel,
} from "./describe";

const NOW = new Date(2026, 9, 15, 15, 0, 0).getTime();
const AT = new Date(2026, 9, 15, 14, 6, 0).getTime();

function revision(fields: Partial<Revision>): Revision {
  return {
    id: "r8",
    created_at: AT,
    origin: "proposal",
    author: null,
    parent: "r7",
    proposal: null,
    pull: null,
    commit: null,
    task: null,
    title: null,
    tree: "abc",
    ...fields,
  };
}

describe("describeRevision", () => {
  it("names the proposal, the task and the agent for a proposal integration", () => {
    expect(
      describeRevision(
        revision({
          origin: "proposal",
          proposal: "P-5",
          task: "T-15",
          title: "Form field components",
          author: "frontend/forms",
        }),
        NOW,
      ),
    ).toEqual({
      title: "P-5 · Form field components",
      meta: ["T-15 · ", { actor: "frontend/forms" }, " · 14:06:00"],
    });
  });

  it("leaves out the task and names an unknown author when the revision has none", () => {
    expect(
      describeRevision(revision({ proposal: "P-5", title: "x" }), NOW).meta,
    ).toEqual(["unknown · 14:06:00"]);
  });

  it("names only the proposal when the revision carries no title", () => {
    expect(describeRevision(revision({ proposal: "P-5" }), NOW).title).toBe(
      "P-5",
    );
  });

  it("names pull, the remote commit and the human for a pull", () => {
    expect(
      describeRevision(
        revision({ origin: "pull", pull: "9f2c1ab", author: "human:tanim" }),
        NOW,
      ),
    ).toEqual({
      title: "pull · 9f2c1ab",
      meta: [{ actor: "human:tanim" }, " · 14:06:00"],
    });
  });

  it("shows the title of the first revision, or a name when it has none", () => {
    expect(
      describeRevision(
        revision({
          origin: "initial",
          title: "Imported",
          author: "human:tanim",
        }),
        NOW,
      ).title,
    ).toBe("Imported");
    expect(describeRevision(revision({ origin: "initial" }), NOW).title).toBe(
      "Initial revision",
    );
  });
});

describe("isOpenable", () => {
  it("opens an entry with a UTF-8 name", () => {
    expect(isOpenable({ name: "a.md", path: "docs/a.md" })).toBe(true);
  });

  it("never opens an entry whose name came through lossy", () => {
    expect(isOpenable({ name: "a\uFFFD.md", path: "docs/a\uFFFD.md" })).toBe(
      false,
    );
    expect(isOpenable({ name: "ok", path: "d\uFFFD/ok" })).toBe(false);
  });
});

describe("fileState", () => {
  const file = (fields: Partial<FileView>): FileView => ({
    path: "a",
    kind: "file",
    size: 3,
    hash: "h",
    binary: false,
    too_large: false,
    text: "abc",
    ...fields,
  });

  it("shows the text of a text file", () => {
    expect(fileState(file({}))).toEqual({ kind: "text", text: "abc" });
    expect(fileState(file({ text: null }))).toEqual({ kind: "text", text: "" });
  });

  it("shows the target of a symlink", () => {
    expect(fileState(file({ kind: "symlink", text: "../a" }))).toEqual({
      kind: "symlink",
      target: "../a",
    });
  });

  it("shows no content for a binary or too-large file", () => {
    expect(fileState(file({ binary: true, text: null }))).toEqual({
      kind: "binary",
    });
    expect(fileState(file({ too_large: true, text: null }))).toEqual({
      kind: "too_large",
    });
  });

  it("calls a file too large before it asks whether it is binary", () => {
    expect(fileState(file({ binary: true, too_large: true })).kind).toBe(
      "too_large",
    );
  });
});

describe("formatSize", () => {
  it("scales bytes, kilobytes and megabytes", () => {
    expect(formatSize(12)).toBe("12 B");
    expect(formatSize(18_432)).toBe("18.0 KB");
    expect(formatSize(3 * 1024 * 1024)).toBe("3.0 MB");
  });
});

describe("symlinkLabel", () => {
  it("shows a symlink's target when the entry carries it", () => {
    expect(symlinkLabel({ kind: "symlink", target: "../a.ts" })).toBe(
      "→ ../a.ts",
    );
  });

  it("shows nothing without a target or for another kind", () => {
    expect(symlinkLabel({ kind: "symlink" })).toBeNull();
    expect(symlinkLabel({ kind: "file", target: "x" })).toBeNull();
  });
});

describe("mainCopyText", () => {
  it("names the head the repository route sends", () => {
    expect(
      mainCopyText({ head: "r17", main_path: "/srv/demo/main", recent: [] }),
    ).toBe(
      "main/ on disk always matches r17 and is read-only. Agents read it; builds run in workspaces.",
    );
  });

  it("says nothing before the read arrives or while main has no revision", () => {
    expect(mainCopyText(undefined)).toBeNull();
    expect(
      mainCopyText({ head: null, main_path: "/srv/demo/main", recent: [] }),
    ).toBeNull();
  });
});
