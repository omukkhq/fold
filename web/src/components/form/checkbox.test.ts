import { describe, expect, it } from "vitest";
import { checkboxClasses, isChecked } from "./checkbox";

describe("isChecked", () => {
  it("reads only a true state as checked", () => {
    expect(isChecked(true)).toBe(true);
    expect(isChecked(false)).toBe(false);
    expect(isChecked("indeterminate")).toBe(false);
  });
});

describe("checkboxClasses", () => {
  it("draws the box from tokens", () => {
    const classes = checkboxClasses.split(" ");
    expect(classes).toContain("size-16");
    expect(classes).toContain("rounded-s");
    expect(classes).toContain("data-[state=checked]:bg-primary");
    expect(classes).toContain("text-on-primary");
  });
});
