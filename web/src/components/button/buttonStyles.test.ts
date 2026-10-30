import { describe, expect, it } from "vitest";
import {
  DISABLED_CLASSES,
  buttonClasses,
  buttonIconClass,
  iconButtonClasses,
  iconPixels,
  controlLabel,
  isInert,
  type ButtonVariant,
} from "./buttonStyles";

const VARIANTS: ButtonVariant[] = ["primary", "quiet", "ghost", "danger"];

describe("the disabled look", () => {
  it("is the same for every button and icon button", () => {
    for (const variant of VARIANTS) {
      expect(buttonClasses(variant, "md")).toContain(DISABLED_CLASSES);
      expect(iconButtonClasses(variant, "sm")).toContain(DISABLED_CLASSES);
    }
  });
});

describe("buttonClasses", () => {
  it.each(VARIANTS)(
    "styles %s from tokens and differs from the others",
    (variant) => {
      const classes = buttonClasses(variant, "md");
      expect(classes).toContain("rounded-s");
      expect(classes).toContain("font-title");
      expect(classes).not.toMatch(/inset-ring|border/);
      const others = VARIANTS.filter((other) => other !== variant).map(
        (other) => buttonClasses(other, "md"),
      );
      expect(others).not.toContain(classes);
    },
  );

  it("makes the small size smaller", () => {
    expect(buttonClasses("ghost", "sm")).toContain("text-13");
    expect(buttonClasses("ghost", "md")).toContain("text-14");
  });
});

describe("button geometry", () => {
  it("pads a ghost button 10 and the others 14 across", () => {
    expect(buttonClasses("ghost", "md")).toContain("px-10");
    for (const variant of ["primary", "quiet", "danger"] as const) {
      expect(buttonClasses(variant, "md")).toContain("px-14");
    }
  });

  it("sets a quiet button on the fill tone, with no stroke", () => {
    expect(buttonClasses("quiet", "md")).toContain("bg-fill");
    expect(buttonClasses("quiet", "md")).not.toContain("inset-ring");
  });
});

describe("buttonIconClass", () => {
  it("draws only the quiet icon in the secondary tone", () => {
    expect(buttonIconClass("quiet")).toContain("text-text-2");
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
    expect(iconPixels("md")).toBe(16);
    expect(iconPixels("sm")).toBe(14);
  });
});

describe("isInert", () => {
  it("is true when disabled or pending", () => {
    expect(isInert(true, false)).toBe(true);
    expect(isInert(false, true)).toBe(true);
    expect(isInert(undefined, undefined)).toBe(false);
  });
});

describe("controlLabel", () => {
  it("is the pending label only while pending", () => {
    expect(controlLabel("Save", true, "Saving…")).toBe("Saving…");
    expect(controlLabel("Save", false, "Saving…")).toBe("Save");
    expect(controlLabel("Save", undefined, undefined)).toBe("Save");
    expect(controlLabel("Save", true, undefined)).toBe("Save");
  });
});
