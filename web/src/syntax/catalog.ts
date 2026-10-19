import type { BundledLanguage } from "shiki/langs";
import { EXTENSIONS, NAMES } from "./catalog.gen";
import type { LanguageSource } from "./languages";

/** Files known by their whole name, which Shiki does not map. Ignore files read as CODEOWNERS: a path pattern per line and `#` comments. */
const FILE_NAMES: Readonly<Record<string, BundledLanguage>> = {
  ".babelrc": "jsonc",
  ".bash_logout": "shellscript",
  ".bash_profile": "shellscript",
  ".bashrc": "shellscript",
  ".dockerignore": "codeowners",
  ".editorconfig": "ini",
  ".env": "dotenv",
  ".envrc": "shellscript",
  ".eslintignore": "codeowners",
  ".eslintrc": "jsonc",
  ".gitconfig": "ini",
  ".gitignore": "codeowners",
  ".gitmodules": "ini",
  ".hgignore": "codeowners",
  ".npmignore": "codeowners",
  ".npmrc": "ini",
  ".prettierignore": "codeowners",
  ".profile": "shellscript",
  ".zprofile": "shellscript",
  ".zshenv": "shellscript",
  ".zshrc": "shellscript",
  brewfile: "ruby",
  "cargo.lock": "toml",
  codeowners: "codeowners",
  commit_editmsg: "git-commit",
  containerfile: "docker",
  dockerfile: "docker",
  gemfile: "ruby",
  "git-rebase-todo": "git-rebase",
  gnumakefile: "make",
  guardfile: "ruby",
  jenkinsfile: "groovy",
  "jsconfig.json": "jsonc",
  justfile: "just",
  makefile: "make",
  merge_msg: "git-commit",
  pipfile: "toml",
  pkgbuild: "shellscript",
  podfile: "ruby",
  "poetry.lock": "toml",
  rakefile: "ruby",
  "tsconfig.json": "jsonc",
  "uv.lock": "toml",
  vagrantfile: "ruby",
};

/** Names such as `Dockerfile.dev` and `.env.local`: a prefix and any rest. */
const PREFIXES: readonly (readonly [string, BundledLanguage])[] = [
  ["containerfile.", "docker"],
  ["dockerfile.", "docker"],
  [".env.", "dotenv"],
];

/** Common extensions that Shiki does not give, or that it gives to a grammar most such files are not written in. */
const EXTENSIONS_OVERRIDE: Readonly<Record<string, BundledLanguage>> = {
  bats: "shellscript",
  bib: "bibtex",
  cc: "cpp",
  cfg: "ini",
  cljc: "clojure",
  cljs: "clojure",
  "code-workspace": "jsonc",
  csproj: "xml",
  csx: "csharp",
  cxx: "cpp",
  edn: "clojure",
  env: "dotenv",
  fsi: "fsharp",
  fsproj: "xml",
  fsx: "fsharp",
  gemspec: "ruby",
  geojson: "json",
  gradle: "groovy",
  h: "c",
  "h++": "cpp",
  hh: "cpp",
  hpp: "cpp",
  htm: "html",
  hxx: "cpp",
  ino: "cpp",
  ksh: "shellscript",
  mdown: "markdown",
  mk: "make",
  mkd: "markdown",
  mm: "objective-cpp",
  ndjson: "jsonl",
  patch: "diff",
  pl: "perl",
  plist: "xml",
  pm: "perl",
  podspec: "ruby",
  props: "xml",
  psd1: "powershell",
  psm1: "powershell",
  pyi: "python",
  pyw: "python",
  rake: "ruby",
  rbi: "ruby",
  ru: "ruby",
  sbt: "scala",
  sc: "scala",
  shtml: "html",
  svg: "xml",
  targets: "xml",
  vcxproj: "xml",
  webmanifest: "json",
  xaml: "xml",
  xhtml: "html",
  xsd: "xml",
  xslt: "xsl",
};

function bySuffix(suffix: string): BundledLanguage | null {
  return (
    EXTENSIONS_OVERRIDE[suffix] ?? NAMES[suffix] ?? EXTENSIONS[suffix] ?? null
  );
}

/**
 * A grammar's file types also hold whole names such as `CMakeLists.txt` and `.htaccess`. A language's id or alias is no
 * whole name, so a file called `go` or `c` stays plain. The longest suffix wins: `a.blade.php` is Blade, not PHP.
 */
function byFileName(name: string): BundledLanguage | null {
  const whole =
    FILE_NAMES[name] ??
    PREFIXES.find(([prefix]) => name.startsWith(prefix))?.[1] ??
    EXTENSIONS[name.replace(/^\./, "")];
  if (whole) return whole;
  for (let dot = name.indexOf(".", 1); dot !== -1; ) {
    const found = bySuffix(name.slice(dot + 1));
    if (found) return found;
    dot = name.indexOf(".", dot + 1);
  }
  return null;
}

/** The Shiki language that `source` names, or null when Shiki has no grammar for it and the code stays plain. */
export function resolveLanguage(
  source: LanguageSource,
): BundledLanguage | null {
  if (source.startsWith("fence:")) return bySuffix(source.slice(6));
  return byFileName(source.slice(5));
}
