import { bundledLanguagesInfo } from "shiki/langs";
import { describe, expect, it } from "vitest";
import { resolveLanguage } from "./catalog";
import { EXTENSIONS, NAMES } from "./catalog.gen";
import { buildCatalog, renderCatalog, shikiLanguages } from "./catalogSource";
import { languageForFence, languageForPath } from "./languages";
import { GRAMMARS } from "./shiki";

function ofPath(path: string) {
  return resolveLanguage(languageForPath(path));
}

describe("resolveLanguage", () => {
  it("knows every language Shiki ships by its id and its aliases", () => {
    expect(bundledLanguagesInfo.length).toBeGreaterThanOrEqual(242);
    for (const { id, aliases = [] } of bundledLanguagesInfo) {
      for (const name of [id, ...aliases]) {
        expect(resolveLanguage(`fence:${name.toLowerCase()}`), name).toBe(id);
      }
    }
  });

  it.each([
    ["src/main.rs", "rust"],
    ["web/src/App.tsx", "tsx"],
    ["a/b/c.d.ts", "typescript"],
    ["lib.MJS", "javascript"],
    ["config.yml", "yaml"],
    ["include/x.hpp", "cpp"],
    ["include/x.h", "c"],
    ["Cargo.toml", "toml"],
    ["README.md", "markdown"],
    ["view.blade.php", "blade"],
    ["index.php", "php"],
    ["lib/parse.ml", "ocaml"],
    ["login.feature", "gherkin"],
    ["script.pl", "perl"],
    ["refs.bib", "bibtex"],
    ["icon.svg", "xml"],
    ["fix.patch", "diff"],
  ])("maps the extension of %s to %s", (path, language) => {
    expect(ofPath(path)).toBe(language);
  });

  it.each([
    ["Makefile", "make"],
    ["deploy/Dockerfile", "docker"],
    ["Dockerfile.dev", "docker"],
    ["Containerfile", "docker"],
    ["CMakeLists.txt", "cmake"],
    ["etc/nginx.conf", "nginx"],
    ["public/.htaccess", "apache"],
    [".gitignore", "codeowners"],
    [".github/CODEOWNERS", "codeowners"],
    [".env", "dotenv"],
    [".env.local", "dotenv"],
    ["home/.zshrc", "shellscript"],
    ["Gemfile", "ruby"],
    ["justfile", "just"],
    ["tsconfig.json", "jsonc"],
  ])("maps the file name %s to %s", (path, language) => {
    expect(ofPath(path)).toBe(language);
  });

  it.each(["notes.txt", "LICENSE", "trailing.", "bin/go", "site.conf", ""])(
    "leaves %j plain",
    (path) => {
      expect(ofPath(path)).toBeNull();
    },
  );

  it("resolves a fence by a name, an alias or an extension", () => {
    const fence = (className: string) => {
      const source = languageForFence(className);
      return source === null ? null : resolveLanguage(source);
    };
    expect(fence("language-TypeScript")).toBe("typescript");
    expect(fence("language-sh")).toBe("shellscript");
    expect(fence("language-c++")).toBe("cpp");
    expect(fence("language-hpp")).toBe("cpp");
    expect(fence("language-feature")).toBe("gherkin");
    expect(fence("language-brainfuck")).toBeNull();
  });
});

describe("catalog.gen.ts", () => {
  it("matches the installed Shiki; run `make gen-languages` after a Shiki update", async () => {
    expect(buildCatalog(await shikiLanguages())).toEqual({
      names: NAMES,
      extensions: EXTENSIONS,
    });
  });
});

describe("buildCatalog", () => {
  it("leaves out a file type that several grammars claim or that names a language", () => {
    const catalog = buildCatalog([
      {
        id: "a",
        aliases: ["A1"],
        fileTypes: [".AX", "shared", "b", "x y", "p/q", ""],
      },
      { id: "b", aliases: [], fileTypes: ["shared", "bx.conf"] },
    ]);
    expect(catalog).toEqual({
      names: { a: "a", a1: "a", b: "b" },
      extensions: { ax: "a", "bx.conf": "b" },
    });
    const source = renderCatalog(catalog);
    expect(source).toContain("export const NAMES");
    expect(source).toContain('  "bx.conf": "b",');
  });
});

describe("GRAMMARS", () => {
  it("loads every language with the grammar Shiki ships for it", () => {
    const grammars = new Map<string, unknown>(Object.entries(GRAMMARS));
    for (const info of bundledLanguagesInfo) {
      expect(grammars.get(info.id), info.id).toBe(info.import);
    }
  });
});
