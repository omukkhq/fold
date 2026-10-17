import { describe, expect, it } from "vitest";
import {
  LANGUAGES,
  type Language,
  languageForFence,
  languageForPath,
} from "./languages";
import { GRAMMARS } from "./shiki";

describe("languageForPath", () => {
  it.each<[string, Language | null]>([
    ["src/main.rs", "rust"],
    ["web/src/App.tsx", "tsx"],
    ["a/b/c.ts", "typescript"],
    ["lib.MJS", "javascript"],
    ["config.yml", "yaml"],
    ["include/x.hpp", "cpp"],
    ["Makefile", "make"],
    ["deploy/Dockerfile", "docker"],
    ["Gemfile", "ruby"],
    ["home/.zshrc", "shellscript"],
    ["Cargo.toml", "toml"],
    ["README.md", "markdown"],
    ["notes.txt", null],
    ["LICENSE", null],
    [".gitignore", null],
    ["trailing.", null],
  ])("%s is %s", (path, language) => {
    expect(languageForPath(path)).toBe(language);
  });
});

describe("languageForFence", () => {
  it.each<[string | undefined, Language | null]>([
    ["language-ts", "typescript"],
    ["language-TypeScript", "typescript"],
    ["language-sh", "shellscript"],
    ["language-rust", "rust"],
    ["language-c++", "cpp"],
    ["hljs language-py", "python"],
    ["language-brainfuck", null],
    ["", null],
    [undefined, null],
  ])("%j is %s", (className, language) => {
    expect(languageForFence(className)).toBe(language);
  });
});

describe("GRAMMARS", () => {
  it("loads, for every language, the grammar Shiki knows by that name", async () => {
    expect(Object.keys(GRAMMARS)).toEqual([...LANGUAGES]);
    for (const language of LANGUAGES) {
      const { default: registrations } = await GRAMMARS[language]();
      expect(registrations.at(-1)?.name, language).toBe(language);
    }
  });
});
