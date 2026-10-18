import { describe, expect, it } from "vitest";
import {
  type LanguageSource,
  languageForFence,
  languageForPath,
} from "./languages";

describe("languageForPath", () => {
  it.each<[string, LanguageSource]>([
    ["src/main.rs", "file:main.rs"],
    ["deploy/Dockerfile", "file:dockerfile"],
    ["lib.MJS", "file:lib.mjs"],
    ["", "file:"],
  ])("%j is %s", (path, source) => {
    expect(languageForPath(path)).toBe(source);
  });
});

describe("languageForFence", () => {
  it.each<[string | undefined, LanguageSource | null]>([
    ["language-ts", "fence:ts"],
    ["language-TypeScript", "fence:typescript"],
    ["hljs language-c++", "fence:c++"],
    ["", null],
    [undefined, null],
  ])("%j is %s", (className, source) => {
    expect(languageForFence(className)).toBe(source);
  });
});
