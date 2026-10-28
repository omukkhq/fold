import { describe, expect, it } from "vitest";
import { composite, contrastRatio, parseHex } from "../theme/contrast";
import { readSources } from "../test/sources";
import fontsCss from "./fonts.css?raw";
import themeCss from "./index.css?raw";
import tokensCss from "./tokens.css?raw";

const sources = readSources(import.meta.url, "", ["ts", "tsx", "css"]);

const SYNTAX = [
  "keyword",
  "string",
  "comment",
  "number",
  "function",
  "type",
  "constant",
  "punctuation",
];
/** Code is drawn on these fills: the file view, Markdown code blocks and every diff row state. */
const CODE_FILLS = [
  "surface",
  "bg",
  "sunken",
  "diff-add",
  "diff-del",
  "diff-context",
  "diff-selected",
];

const DESIGN_COLOURS = [
  "bg",
  "surface",
  "sunken",
  "line",
  "fill",
  "text",
  "text-2",
  "accent",
  "accent-soft",
  "warn",
  "warn-soft",
  "ok",
  "ok-soft",
  "bad",
  "bad-soft",
  "ink",
  "on-ink",
  "scrim",
  "shade-near",
  "shade-far",
  "status-backlog",
  "status-todo",
  "status-progress",
  "status-review",
  "status-done",
  "diff-add",
  "diff-del",
  "diff-context",
  "diff-hunk",
  "diff-selected",
  "diff-comment-marker",
  ...["sage", "slate", "teal", "ink", "gold", "moss", "plum", "stone"].flatMap(
    (hue) => [`label-${hue}`, `label-${hue}-soft`],
  ),
  "presence-idle",
  "presence-offline",
  ...SYNTAX.map((kind) => `syntax-${kind}`),
];
const DESIGN_OTHER = [
  "title",
  "body",
  "mono",
  ...[12, 13, 14, 15, 16, 18, 21, 24, 28].map((n) => `fs-${String(n)}`),
  "lh",
  "lh-tight",
  "r-s",
  "r-m",
  "r-l",
  "r-pill",
  "measure",
  "tracking-wordmark",
  "height-bar",
  "height-bar-compact",
  "width-skeleton-s",
  "width-skeleton-l",
  "width-diff-number",
  "width-search",
  "width-file-list",
  "width-side-panel",
  "width-content",
  "width-history",
  "width-sign-in",
  "width-thread-list",
  "width-dialog",
  "width-review-panel",
  "width-event-stream",
  "width-inbox",
  "width-agents",
  "width-menu",
  ...[
    0, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 13, 14, 16, 18, 20, 22, 24, 28, 32, 40,
    44,
  ].map((n) => `space-${String(n)}`),
];

function blocks(css: string): { selector: string; body: string }[] {
  const out: { selector: string; body: string }[] = [];
  const re = /([^{}]+)\{([^{}]*)\}/g;
  for (const m of css.matchAll(re)) {
    out.push({ selector: (m[1] ?? "").trim(), body: m[2] ?? "" });
  }
  return out;
}

function decls(body: string): Map<string, string> {
  const map = new Map<string, string>();
  for (const m of body.matchAll(/--([\w-]+)\s*:\s*([^;]+);/g)) {
    map.set(m[1] ?? "", (m[2] ?? "").trim());
  }
  return map;
}

const all = blocks(tokensCss);
const find = (selector: string) => {
  const b = all.find(
    (x) => x.selector === selector && x.body.includes("color-scheme"),
  );
  if (!b) throw new Error(`missing block ${selector}`);
  return decls(b.body);
};
const light = find(":root");
const dark = find(':root[data-theme="dark"]');
const auto = find(':root:not([data-theme="light"])');
const root = new Map<string, string>();
for (const b of all)
  if (b.selector === ":root")
    for (const [k, v] of decls(b.body)) root.set(k, v);

describe("token contract", () => {
  it("defines the same names in light, dark and the system-dark block", () => {
    const names = [...light.keys()].sort();
    expect([...dark.keys()].sort()).toEqual(names);
    expect([...auto.keys()].sort()).toEqual(names);
  });
  it("carries every design variable", () => {
    for (const n of DESIGN_COLOURS) expect(light.has(n), n).toBe(true);
    for (const n of DESIGN_OTHER) expect(root.has(n), n).toBe(true);
  });
  it("defines the elevations once, outside the theme blocks, with the popover's shades taken from each theme", () => {
    expect(root.get("elevation-pop")?.replace(/\s+/g, " ")).toBe(
      "0 0 0 1px var(--line), 0 4px 12px var(--shade-near), 0 18px 44px var(--shade-far)",
    );
    expect(root.get("elevation-dialog")).toBe("0 16px 48px #0a0a0b40");
    for (const set of [light, dark, auto]) {
      expect(set.has("elevation-pop")).toBe(false);
      expect(set.has("elevation-dialog")).toBe(false);
    }
  });
  it("keeps the type scale to nine sizes, 12px only for count badges", () => {
    const sizes = [...root.keys()].filter((k) => k.startsWith("fs-"));
    expect(sizes).toEqual(
      [12, 13, 14, 15, 16, 18, 21, 24, 28].map((n) => `fs-${String(n)}`),
    );
    for (const k of sizes) expect(root.get(k)).toBe(`${k.slice(3)}px`);
  });
  it("keeps system-dark values equal to the explicit dark values", () => {
    expect(auto).toEqual(dark);
  });
  it("has hex values for every colour", () => {
    for (const set of [light, dark]) {
      for (const [k, v] of set)
        expect(v, k).toMatch(/^#[0-9a-f]{6}([0-9a-f]{2})?$/i);
    }
  });
});

describe("fonts", () => {
  it("bundles the body face in the weights the screens use", () => {
    const weights = [
      ...fontsCss.matchAll(/atkinson-hyperlegible-next\/latin-(\d+)\.css/g),
    ].map((m) => m[1]);
    expect(weights).toEqual(["400", "500", "600"]);
  });
  it("sets the body in Atkinson Hyperlegible Next and the titles in Barlow", () => {
    expect(root.get("body")).toMatch(/^"Atkinson Hyperlegible Next"/);
    expect(root.get("title")).toMatch(/^"Barlow"/);
  });
  it("bundles JetBrains Mono in the weights the screens use", () => {
    const weights = [
      ...fontsCss.matchAll(/jetbrains-mono\/latin-(\d+)\.css/g),
    ].map((m) => m[1]);
    expect(weights).toEqual(["400", "600", "700"]);
  });
  it("turns the mono face's ligatures and contextual alternates off, so code reads as typed", () => {
    const theme = /@theme[^{]*\{([^}]*)\}/.exec(themeCss)?.[1] ?? "";
    const settings =
      /--font-mono--font-feature-settings:\s*([^;]+);/.exec(theme)?.[1] ?? "";
    expect(settings).toMatch(/"calt" 0/);
    expect(settings).toMatch(/"liga" 0/);
  });
});

describe("raw literal guard", () => {
  const files = Object.entries(sources).filter(
    ([p]) =>
      !p.endsWith("/styles/tokens.css") &&
      !p.endsWith(".test.ts") &&
      !p.endsWith(".gen.ts"),
  );
  it("scans source files", () => {
    expect(files.length).toBeGreaterThan(3);
  });
  it("finds no colour literal or remote URL outside tokens.css", () => {
    const bad = /#[0-9a-f]{3,8}\b|\brgba?\(|\bhsla?\(|\boklch\(|https?:\/\//i;
    for (const [path, text] of files) {
      expect(text.match(bad), path).toBeNull();
    }
  });
});

describe("contrast", () => {
  const pairs: [string, string][] = [
    ["text", "surface"],
    ["text", "bg"],
    ["text", "sunken"],
    ["text", "fill"],
    ["text-2", "surface"],
    ["text-2", "bg"],
    ["text-2", "sunken"],
    ["text-2", "fill"],
    ["text-2", "warn-soft"],
    ["text-2", "accent-soft"],
    ["text-2", "diff-context"],
    ["text-2", "diff-hunk"],
    ["text-2", "diff-selected"],
    ["accent", "surface"],
    ["accent", "accent-soft"],
    ["warn", "surface"],
    ["warn", "warn-soft"],
    ["ok", "surface"],
    ["ok", "ok-soft"],
    ["bad", "surface"],
    ["bad", "bad-soft"],
    ["on-ink", "ink"],
    ...["sage", "slate", "teal", "ink", "gold", "moss", "plum", "stone"].map(
      (hue): [string, string] => [`label-${hue}`, `label-${hue}-soft`],
    ),
    ...SYNTAX.flatMap((kind) =>
      CODE_FILLS.map((fill): [string, string] => [`syntax-${kind}`, fill]),
    ),
  ];
  const nonText: [string, string][] = [
    ["accent", "bg"],
    ["accent", "surface"],
    ["presence-idle", "surface"],
    ["presence-offline", "surface"],
    ["diff-comment-marker", "surface"],
  ];
  const ratio = (set: Map<string, string>, fg: string, bg: string) => {
    const surface = parseHex(set.get("surface") ?? "");
    const under = composite(parseHex(set.get(bg) ?? ""), surface);
    const top = composite(parseHex(set.get(fg) ?? ""), under);
    return contrastRatio(top, under);
  };
  for (const [theme, set] of [
    ["light", light],
    ["dark", dark],
  ] as const) {
    for (const [fg, bg] of pairs) {
      it(`${theme}: ${fg} on ${bg} is at least 4.5:1`, () => {
        expect(ratio(set, fg, bg)).toBeGreaterThanOrEqual(4.5);
      });
    }
    for (const [fg, bg] of nonText) {
      it(`${theme}: ${fg} on ${bg} is at least 3:1`, () => {
        expect(ratio(set, fg, bg)).toBeGreaterThanOrEqual(3);
      });
    }
  }
});
