import { describe, expect, it } from "vitest";
import { composite, contrastRatio, parseHex } from "../theme/contrast";
import tokensCss from "./tokens.css?raw";

const sources = import.meta.glob<string>("/src/**/*.{ts,tsx,css}", {
  query: "?raw",
  import: "default",
  eager: true,
});

const W00_COLOURS = [
  "bg",
  "surface",
  "sunken",
  "raised",
  "border",
  "border-subtle",
  "text",
  "text-muted",
  "text-faint",
  "primary",
  "primary-strong",
  "primary-soft",
  "on-primary",
  "primary-muted",
  "primary-text",
  "secondary",
  "secondary-soft",
  "secondary-text",
  "success",
  "success-soft",
  "danger",
  "danger-soft",
  "status-backlog",
  "status-todo",
  "status-progress",
  "status-review",
  "status-done",
  "diff-add",
  "diff-add-strong",
  "diff-del",
  "diff-del-strong",
  "ink",
  "on-ink",
  "label-sage",
  "label-gold",
  "label-moss",
  "label-slate",
  "label-plum",
  "label-teal",
  "label-stone",
  "label-ink",
];
const W00_OTHER = ["font-ui", "font-mono", "radius-s", "radius-m", "radius-l"];

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
  it("carries every W00 variable", () => {
    for (const n of W00_COLOURS) expect(light.has(n), n).toBe(true);
    for (const n of W00_OTHER) expect(root.has(n), n).toBe(true);
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
    ["text-muted", "surface"],
    ["text-muted", "bg"],
    ["text-faint", "surface"],
    ["primary-text", "primary-soft"],
    ["secondary-text", "secondary-soft"],
    ["on-ink", "ink"],
  ];
  for (const [theme, set] of [
    ["light", light],
    ["dark", dark],
  ] as const) {
    for (const [fg, bg] of pairs) {
      it(`${theme}: ${fg} on ${bg} is at least 4.5:1`, () => {
        const surface = parseHex(set.get("surface") ?? "");
        const under = composite(parseHex(set.get(bg) ?? ""), surface);
        const top = composite(parseHex(set.get(fg) ?? ""), under);
        expect(contrastRatio(top, under)).toBeGreaterThanOrEqual(4.5);
      });
    }
  }
});
