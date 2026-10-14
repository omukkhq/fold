import { describe, expect, it } from "vitest";
import { toggledTheme } from "./toggledTheme";

describe("toggledTheme", () => {
  it.each([
    ["light", false, "dark"],
    ["light", true, "dark"],
    ["dark", false, "light"],
    ["dark", true, "light"],
    ["system", false, "dark"],
    ["system", true, "light"],
  ] as const)("%s with system dark %s becomes %s", (choice, dark, next) => {
    expect(toggledTheme(choice, dark)).toBe(next);
  });
});
