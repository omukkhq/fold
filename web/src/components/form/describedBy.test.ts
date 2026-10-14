import { describe, expect, it } from "vitest";
import { describedBy, fieldIds } from "./describedBy";

describe("fieldIds", () => {
  it("derives the hint and error ids from the input id", () => {
    expect(fieldIds("f1")).toEqual({
      input: "f1",
      hint: "f1-hint",
      error: "f1-error",
    });
  });
});

describe("describedBy", () => {
  const ids = fieldIds("f1");

  it("is undefined without hint or error", () => {
    expect(describedBy(ids, {})).toBeUndefined();
  });

  it("lists the hint, then the error", () => {
    expect(describedBy(ids, { hint: true })).toBe("f1-hint");
    expect(describedBy(ids, { error: true })).toBe("f1-error");
    expect(describedBy(ids, { hint: true, error: true })).toBe(
      "f1-hint f1-error",
    );
  });
});
