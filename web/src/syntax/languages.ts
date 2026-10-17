/** The languages fold carries a grammar for; the grammars themselves load on first use. */
export const LANGUAGES = [
  "c",
  "cpp",
  "csharp",
  "css",
  "dart",
  "diff",
  "docker",
  "elixir",
  "go",
  "graphql",
  "hcl",
  "html",
  "ini",
  "java",
  "javascript",
  "json",
  "jsonc",
  "jsx",
  "kotlin",
  "lua",
  "make",
  "markdown",
  "php",
  "proto",
  "python",
  "ruby",
  "rust",
  "scss",
  "shellscript",
  "sql",
  "svelte",
  "swift",
  "terraform",
  "toml",
  "tsx",
  "typescript",
  "vue",
  "xml",
  "yaml",
  "zig",
] as const;

export type Language = (typeof LANGUAGES)[number];

/** Other names a Markdown fence or a file extension gives a language. */
const ALIASES: Record<string, Language> = {
  bash: "shellscript",
  "c#": "csharp",
  "c++": "cpp",
  cc: "cpp",
  cjs: "javascript",
  cs: "csharp",
  cts: "typescript",
  cxx: "cpp",
  dockerfile: "docker",
  ex: "elixir",
  exs: "elixir",
  gql: "graphql",
  golang: "go",
  h: "c",
  hh: "cpp",
  hpp: "cpp",
  htm: "html",
  hxx: "cpp",
  js: "javascript",
  json5: "jsonc",
  kt: "kotlin",
  kts: "kotlin",
  makefile: "make",
  md: "markdown",
  mjs: "javascript",
  mk: "make",
  mts: "typescript",
  patch: "diff",
  protobuf: "proto",
  py: "python",
  pyi: "python",
  rb: "ruby",
  rs: "rust",
  sh: "shellscript",
  shell: "shellscript",
  svg: "xml",
  tf: "terraform",
  tfvars: "terraform",
  ts: "typescript",
  yml: "yaml",
  zsh: "shellscript",
};

/** Files known by their whole name rather than an extension. */
const FILE_NAMES: Record<string, Language> = {
  ".bashrc": "shellscript",
  ".zshrc": "shellscript",
  dockerfile: "docker",
  gemfile: "ruby",
  gnumakefile: "make",
  makefile: "make",
  rakefile: "ruby",
};

function isLanguage(name: string): name is Language {
  return (LANGUAGES as readonly string[]).includes(name);
}

function resolve(name: string): Language | null {
  const key = name.toLowerCase();
  if (isLanguage(key)) return key;
  return ALIASES[key] ?? null;
}

/** The language of a file from its name, or null when fold carries no grammar for it. */
export function languageForPath(path: string): Language | null {
  const name = (path.split("/").pop() ?? "").toLowerCase();
  const byName = FILE_NAMES[name];
  if (byName) return byName;
  const dot = name.lastIndexOf(".");
  if (dot <= 0 || dot === name.length - 1) return null;
  return resolve(name.slice(dot + 1));
}

/** The language a Markdown fence names, from the `language-<info>` class the Markdown renderer gives a fenced block. */
export function languageForFence(
  className: string | undefined,
): Language | null {
  const info = /(?:^|\s)language-(\S+)/.exec(className ?? "")?.[1];
  return info === undefined ? null : resolve(info);
}
