import { describe, expect, it } from "vitest";
import { LABEL_NAMES, labelColors } from "./labelColors";

describe("labelColors", () => {
  it.each(LABEL_NAMES)("maps %s to its own token pair", (name) => {
    expect(labelColors(name)).toEqual({
      name,
      dot: `bg-label-${name}`,
      soft: `bg-label-${name}-soft`,
    });
  });

  it("falls back to the stone pair for an unknown name", () => {
    expect(labelColors("magenta")).toEqual(labelColors("stone"));
    expect(labelColors("")).toEqual(labelColors("stone"));
  });

  it("is case sensitive", () => {
    expect(labelColors("Sage").name).toBe("stone");
  });
});
