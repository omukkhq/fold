import { describe, expect, it } from "vitest";
import {
  buttonClasses,
  buttonIconClass,
  iconButtonClasses,
  iconPixels,
  isInert,
  type ButtonVariant,
} from "./buttonStyles";

const VARIANTS: ButtonVariant[] = ["primary", "secondary", "ghost", "danger"];

describe("buttonClasses", () => {
  it.each(VARIANTS)(
    "styles %s from tokens and differs from the others",
    (variant) => {
      const classes = buttonClasses(variant, "md");
      expect(classes).toContain("rounded-m");
      const others = VARIANTS.filter((other) => other !== variant).map(
        (other) => buttonClasses(other, "md"),
      );
      expect(others).not.toContain(classes);
    },
  );

  it("makes the small size smaller", () => {
    expect(buttonClasses("ghost", "sm")).toContain("text-code");
    expect(buttonClasses("ghost", "md")).toContain("text-body");
  });
});

describe("button geometry", () => {
  it("pads a ghost button 10 and the others 14 across", () => {
    expect(buttonClasses("ghost", "md")).toContain("px-10");
    for (const variant of ["primary", "secondary", "danger"] as const) {
      expect(buttonClasses(variant, "md")).toContain("px-14");
    }
  });

  it("draws the secondary stroke inside the box", () => {
    expect(buttonClasses("secondary", "md")).toContain("inset-ring-border");
    expect(buttonClasses("secondary", "md").split(" ")).not.toContain("border");
  });
});

describe("buttonIconClass", () => {
  it("mutes only the secondary icon", () => {
    expect(buttonIconClass("secondary")).toContain("text-text-muted");
    expect(buttonIconClass("primary")).not.toContain("text-");
    expect(buttonIconClass("ghost")).not.toContain("text-");
  });
});

describe("iconButtonClasses", () => {
  it("is square for both sizes", () => {
    expect(iconButtonClasses("ghost", "md")).toContain("size-32");
    expect(iconButtonClasses("ghost", "sm")).toContain("size-24");
  });
});

describe("iconPixels", () => {
  it("shrinks the icon with the size", () => {
    expect(iconPixels("md")).toBe(15);
    expect(iconPixels("sm")).toBe(13);
  });
});

describe("isInert", () => {
  it("is true when disabled or pending", () => {
    expect(isInert(true, false)).toBe(true);
    expect(isInert(false, true)).toBe(true);
    expect(isInert(undefined, undefined)).toBe(false);
  });
});
