import { describe, expect, it } from "vitest";
import {
  TABLE_CELL,
  TABLE_HEAD,
  TABLE_LINK_ROW,
  TABLE_ROW,
} from "./tableStyles";

describe("table styles", () => {
  it("separates rows by a zebra fill, never by a border", () => {
    for (const row of [TABLE_ROW, TABLE_LINK_ROW]) {
      expect(row).toContain("even:bg-sunken/45");
      expect(row).not.toContain("border");
    }
    expect(TABLE_LINK_ROW).toContain("hover:bg-sunken");
  });

  it("sets heads in sentence case at 13px and cells at 14px", () => {
    expect(TABLE_HEAD).toContain("text-13");
    expect(TABLE_HEAD).not.toMatch(/uppercase/);
    expect(TABLE_CELL).toContain("text-14");
  });
});
