import { describe, expect, it } from "vitest";
import { cn } from "./cn";

describe("cn", () => {
  it("joins truthy parts and drops falsy ones", () => {
    expect(cn("a", false, null, undefined, "", "b")).toBe("a b");
  });

  it("lets a later class of the same group win", () => {
    expect(cn("px-8", "px-12")).toBe("px-12");
  });

  it("keeps a size token and a colour token apart", () => {
    expect(cn("text-13", "text-text-2")).toBe("text-13 text-text-2");
  });

  it("replaces one size token with another", () => {
    expect(cn("text-13", "text-15")).toBe("text-15");
  });

  it("replaces one weight token with another", () => {
    expect(cn("font-medium", "font-bold")).toBe("font-bold");
  });

  it("knows the numbered sizes, so a colour keeps them", () => {
    expect(cn("text-14", "text-text")).toBe("text-14 text-text");
    expect(cn("text-21", "text-text-2")).toBe("text-21 text-text-2");
    expect(cn("text-12", "text-28")).toBe("text-28");
  });

  it("replaces one radius token with another", () => {
    expect(cn("rounded-s", "rounded-m")).toBe("rounded-m");
  });

  it("replaces one inset ring colour with another", () => {
    expect(cn("inset-ring inset-ring-line", "inset-ring-bad")).toBe(
      "inset-ring inset-ring-bad",
    );
  });

  it("keeps a leading that follows the size", () => {
    expect(cn("leading-tight", "text-15")).toBe("text-15");
    expect(cn("text-15 leading-tight")).toBe("text-15 leading-tight");
  });

  it("replaces one elevation token with another", () => {
    expect(cn("shadow-pop", "shadow-dialog")).toBe("shadow-dialog");
  });
});
