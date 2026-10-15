import { describe, expect, it } from "vitest";
import {
  NAME_HINT,
  NAME_MAX_LENGTH,
  nameProblemMessage,
  validateName,
} from "./name";

describe("validateName", () => {
  it("trims a valid name", () => {
    expect(validateName("  tanim  ")).toEqual({ ok: true, name: "tanim" });
  });

  it("accepts digits, dashes and underscores after the first character", () => {
    for (const name of ["ana-2", "r2_d2", "7up", "a"]) {
      expect(validateName(name)).toEqual({ ok: true, name });
    }
  });

  it("rejects an empty or blank name", () => {
    expect(validateName("")).toEqual({ ok: false, problem: "empty" });
    expect(validateName(" \t ")).toEqual({ ok: false, problem: "empty" });
  });

  it("accepts exactly 32 characters and rejects 33", () => {
    expect(NAME_MAX_LENGTH).toBe(32);
    expect(validateName("a".repeat(NAME_MAX_LENGTH)).ok).toBe(true);
    expect(validateName("a".repeat(NAME_MAX_LENGTH + 1))).toEqual({
      ok: false,
      problem: "too_long",
    });
  });

  it("rejects anything outside lowercase letters, digits, dashes and underscores", () => {
    for (const name of [
      "Tanim",
      "tanim sarwar",
      "ta\nnim",
      "ta\u0000nim",
      "tanim.s",
      "tänim",
      "🙂",
    ]) {
      expect(validateName(name)).toEqual({ ok: false, problem: "characters" });
    }
  });

  it("rejects a name that starts with a dash or an underscore", () => {
    expect(validateName("-tanim")).toEqual({ ok: false, problem: "start" });
    expect(validateName("_tanim")).toEqual({ ok: false, problem: "start" });
  });

  it("words every problem and the rule", () => {
    expect(nameProblemMessage("empty")).toBe("Enter a name.");
    expect(nameProblemMessage("too_long")).toContain("32");
    expect(nameProblemMessage("characters")).toContain("lowercase");
    expect(nameProblemMessage("start")).toContain("Start with");
    expect(NAME_HINT).toContain("32");
  });
});
