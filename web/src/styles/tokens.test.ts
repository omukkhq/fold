import { describe, expect, it } from "vitest";
import { composite, contrastRatio, parseHex } from "../theme/contrast";
import { readSources } from "../test/sources";
import fontsCss from "./fonts.css?raw";
import themeCss from "./index.css?raw";
import tokensCss from "./tokens.css?raw";

const sources = readSources(import.meta.url, "", ["ts", "tsx", "css"]);

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
  "warning",
  "warning-soft",
  "focus-ring",
  "scrim",
  "presence-active",
  "presence-idle",
  "presence-offline",
  "ink-soft",
  "label-sage-soft",
  "label-slate-soft",
  "label-teal-soft",
  "label-ink-soft",
  "label-gold-soft",
  "label-moss-soft",
  "label-plum-soft",
  "label-stone-soft",
  "diff-context",
  "diff-hunk",
  "diff-selected",
  "diff-comment-marker",
  "diff-outdated",
];
const W00_OTHER = [
  "font-ui",
  "font-mono",
  "radius-xs",
  "radius-s",
  "radius-m",
  "radius-l",
  "radius-chip",
  "size-micro",
  "size-caption",
  "size-code",
  "size-bar",
  "size-wordmark",
  "size-screen",
  "size-item-title",
  "tracking-wordmark",
  "height-bar",
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
  ...[
    0, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 13, 14, 16, 18, 20, 22, 24, 28, 32, 40,
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
  it("carries every W00 variable", () => {
    for (const n of W00_COLOURS) expect(light.has(n), n).toBe(true);
    for (const n of W00_OTHER) expect(root.has(n), n).toBe(true);
  });
  it("defines the elevations once, outside the theme blocks, as W00 draws them for both themes", () => {
    expect(root.get("elevation-card")).toBe("0 1px 2px #1a1a1f14");
    expect(root.get("elevation-dialog")).toBe("0 16px 48px #0a0a0b40");
    for (const set of [light, dark, auto]) {
      expect(set.has("elevation-card")).toBe(false);
      expect(set.has("elevation-dialog")).toBe(false);
    }
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
  it("bundles JetBrains Mono in the weights the frames use", () => {
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
    ["text-muted", "surface"],
    ["text-muted", "bg"],
    ["text-faint", "surface"],
    ["primary-text", "primary-soft"],
    ["secondary-text", "secondary-soft"],
    ["on-ink", "ink"],
    ["warning", "warning-soft"],
    ["text-muted", "warning-soft"],
    ["text-muted", "ink-soft"],
    ["text-muted", "diff-context"],
    ["text-muted", "diff-hunk"],
    ["text-muted", "diff-selected"],
    ...["sage", "slate", "teal", "ink", "gold", "moss", "plum", "stone"].map(
      (hue): [string, string] => [`label-${hue}`, `label-${hue}-soft`],
    ),
  ];
  const nonText: [string, string][] = [
    ["focus-ring", "bg"],
    ["focus-ring", "surface"],
    ["presence-active", "surface"],
    ["presence-idle", "surface"],
    ["presence-offline", "surface"],
    ["diff-comment-marker", "surface"],
    ["diff-outdated", "surface"],
  ];
  // The product owner accepted these light pairs below 4.5:1, so their exact
  // values are pinned instead of a ratio.
  const acceptedLight: [string, string, string, string][] = [
    ["on-primary", "primary", "#ffffff", "#6b8f8a"],
    ["success", "success-soft", "#6a9e7a", "#f0f7f2"],
    ["text-faint", "sunken", "#72716b", "#f3f1ed"],
  ];
  const darkOnly = acceptedLight.map(([fg, bg]) => [fg, bg] as const);
  const ratio = (set: Map<string, string>, fg: string, bg: string) => {
    const surface = parseHex(set.get("surface") ?? "");
    const under = composite(parseHex(set.get(bg) ?? ""), surface);
    const top = composite(parseHex(set.get(fg) ?? ""), under);
    return contrastRatio(top, under);
  };
  for (const [fg, bg, fgHex, bgHex] of acceptedLight) {
    it(`light: ${fg} on ${bg} keeps its accepted values`, () => {
      expect(light.get(fg)).toBe(fgHex);
      expect(light.get(bg)).toBe(bgHex);
    });
  }
  for (const [fg, bg] of darkOnly) {
    it(`dark: ${fg} on ${bg} is at least 4.5:1`, () => {
      expect(ratio(dark, fg, bg)).toBeGreaterThanOrEqual(4.5);
    });
  }
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
