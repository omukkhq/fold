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
    expect(cn("text-meta", "text-muted")).toBe("text-meta text-muted");
  });

  it("replaces one size token with another", () => {
    expect(cn("text-meta", "text-body")).toBe("text-body");
  });

  it("replaces one weight token with another", () => {
    expect(cn("font-medium", "font-bold")).toBe("font-bold");
  });

  it("knows the bar and wordmark sizes, so a colour keeps them", () => {
    expect(cn("text-bar", "text-text")).toBe("text-bar text-text");
    expect(cn("text-wordmark", "text-muted")).toBe("text-wordmark text-muted");
  });

  it("replaces one radius token with another", () => {
    expect(cn("rounded-s", "rounded-m")).toBe("rounded-m");
  });

  it("replaces one inset ring colour with another", () => {
    expect(cn("inset-ring inset-ring-border", "inset-ring-danger")).toBe(
      "inset-ring inset-ring-danger",
    );
  });

  it("keeps a leading that follows the size", () => {
    expect(cn("leading-snug", "text-body")).toBe("text-body");
    expect(cn("text-body leading-snug")).toBe("text-body leading-snug");
  });

  it("replaces one elevation token with another", () => {
    expect(cn("shadow-card", "shadow-dialog")).toBe("shadow-dialog");
  });
});
