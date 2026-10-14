import { describe, expect, it } from "vitest";
import { cx } from "./cx";

describe("cx", () => {
  it("joins truthy parts with a space", () => {
    expect(cx("a", "b")).toBe("a b");
  });

  it("drops false, null, undefined and empty parts", () => {
    expect(cx("a", false, null, undefined, "", "b")).toBe("a b");
  });

  it("returns an empty string without parts", () => {
    expect(cx()).toBe("");
  });
});
