import { describe, expect, it } from "vitest";
import { isPlainLeftClick, type ClickLike } from "./navClick";

const plain: ClickLike = {
  button: 0,
  metaKey: false,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  defaultPrevented: false,
};

describe("isPlainLeftClick", () => {
  it("accepts a plain left click", () => {
    expect(isPlainLeftClick(plain)).toBe(true);
  });
  it.each([
    { button: 1 },
    { metaKey: true },
    { ctrlKey: true },
    { shiftKey: true },
    { altKey: true },
    { defaultPrevented: true },
  ])("rejects %j", (patch) => {
    expect(isPlainLeftClick({ ...plain, ...patch })).toBe(false);
  });
});
