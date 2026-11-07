// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import {
  STASH_HEADING_ID,
  focusAfterFolderWrite,
  positionToFocus,
} from "./focus";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("positionToFocus", () => {
  it("picks the entry that took the dropped place, else the nearest newer one, else the heading", () => {
    expect(positionToFocus([0, 1, 2], 1)).toBe(1);
    expect(positionToFocus([0, 2], 1)).toBe(2);
    expect(positionToFocus([0, 1], 2)).toBe(1);
    expect(positionToFocus([], 0)).toBeNull();
  });
});

describe("focusAfterFolderWrite", () => {
  function page(positions: number[], heading = true) {
    document.body.innerHTML = `${heading ? `<h2 id="${STASH_HEADING_ID}" tabindex="-1">Stash</h2>` : ""}${positions
      .map(
        (position) =>
          `<button data-stash-position="${String(position)}">Drop</button>`,
      )
      .join("")}`;
  }

  it("focuses the Drop button that took the dropped entry's place", () => {
    page([0, 1]);
    expect(focusAfterFolderWrite(document, 1)).toBe(true);
    expect(document.activeElement?.getAttribute("data-stash-position")).toBe(
      "1",
    );
  });

  it("focuses the stash heading after Clear all, a reset or a restore, or when no entry can be dropped", () => {
    page([]);
    expect(focusAfterFolderWrite(document, 0)).toBe(true);
    expect(document.activeElement?.id).toBe(STASH_HEADING_ID);
    page([0]);
    expect(focusAfterFolderWrite(document, null)).toBe(true);
    expect(document.activeElement?.id).toBe(STASH_HEADING_ID);
  });

  it("leaves focus to the dialog when the stash is gone from the page", () => {
    page([], false);
    expect(focusAfterFolderWrite(document, null)).toBe(false);
  });
});
