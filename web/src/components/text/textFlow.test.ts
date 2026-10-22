import { describe, expect, it } from "vitest";
import { CODE_BOX, textFlow } from "./textFlow";

describe("textFlow", () => {
  it("wraps written text and breaks a long unbroken run", () => {
    expect(textFlow("prose")).toBe("whitespace-pre-wrap wrap-anywhere");
  });

  it("folds the spaces of an excerpt and still breaks a long run", () => {
    expect(textFlow("lines")).toBe("whitespace-pre-line wrap-anywhere");
  });

  it("keeps each line of code on one line", () => {
    expect(textFlow("code")).toBe("whitespace-pre");
  });

  it("lets the box of code scroll and shrink", () => {
    expect(CODE_BOX.split(" ")).toEqual(
      expect.arrayContaining(["min-w-0", "overflow-x-auto"]),
    );
  });
});
