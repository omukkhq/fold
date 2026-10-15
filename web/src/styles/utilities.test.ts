import { describe, expect, it } from "vitest";
import themeCss from "./index.css?raw";

const sources = import.meta.glob<string>("/src/**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
});

type Keys = Map<string, Set<string>>;

// Theme namespaces whose name has a dash; the longest match wins, so `--max-width-x` is the key `x` of `max-width`, not `width-x` of `max`.
const COMPOUND = [
  "font-weight",
  "min-width",
  "max-width",
  "min-height",
  "max-height",
  "flex-basis",
];

function splitName(name: string): [string, string] {
  const compound = COMPOUND.find((ns) => name.startsWith(`${ns}-`));
  if (compound) return [compound, name.slice(compound.length + 1)];
  const dash = name.indexOf("-");
  return [name.slice(0, dash), name.slice(dash + 1)];
}

function themeKeys(css: string): Keys {
  const body = /@theme[^{]*\{([^}]*)\}/.exec(css)?.[1] ?? "";
  const keys: Keys = new Map();
  for (const m of body.matchAll(/(?<![\w-])--([a-z][\w.-]*)\s*:/g)) {
    const name = m[1] ?? "";
    if (name.includes("--") || !name.includes("-")) continue;
    const [ns, key] = splitName(name);
    const set = keys.get(ns) ?? new Set<string>();
    set.add(key);
    keys.set(ns, set);
  }
  return keys;
}

const SPACING = [
  ..."p px py ps pe pt pr pb pl m mx my ms me mt mr mb ml".split(" "),
  ..."gap gap-x gap-y space-x space-y".split(" "),
  ..."inset inset-x inset-y top right bottom left".split(" "),
];
// The theme namespaces Tailwind 4.3 reads for each sizing utility, in its own order.
const SIZING: Record<string, string[]> = {
  w: ["width", "spacing", "container"],
  "min-w": ["min-width", "spacing", "container"],
  "max-w": ["max-width", "spacing", "container"],
  h: ["height", "spacing"],
  "min-h": ["min-height", "height", "spacing"],
  "max-h": ["max-height", "height", "spacing"],
  size: ["size", "spacing"],
  basis: ["flex-basis", "spacing", "container"],
};
// Named min and max sizes rarely share a shape with ids or paths, so their names are checked against the theme too; these are Tailwind's static values.
const NAMED_CHECKED = new Set(["min-w", "max-w", "min-h", "max-h"]);
const SIZE_KEYWORDS = new Set(
  "auto full screen svw lvw dvw svh lvh dvh lh min max fit none px prose".split(
    " ",
  ),
);
const FRACTIONS = new Set([
  ...Object.keys(SIZING),
  ..."inset inset-x inset-y top right bottom left".split(" "),
]);
const TEXT_KEYWORDS = new Set(
  "left center right justify start end wrap nowrap balance pretty ellipsis clip".split(
    " ",
  ),
);

const prefixes = [...SPACING, ...Object.keys(SIZING), "text"].sort(
  (a, b) => b.length - a.length,
);
const candidate = new RegExp(
  String.raw`(?<![\w./-])(?:[\w-]+:)*!?-?(${prefixes.join("|")})-(\[[^\]\s]*\]|[\w.%/-]+)!?(?![\w-])`,
  "g",
);

/**
 * Tailwind drops a utility whose value is not a theme key without an error,
 * so a value outside the token scale has to be caught here. Named values on
 * spacing and sizing utilities are not checked, because ids, paths and token
 * names in strings share their shape.
 */
function offToken(text: string, keys: Keys): string[] {
  const has = (namespaces: string[], value: string) =>
    namespaces.some((ns) => keys.get(ns)?.has(value));
  const code = text
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/(^|[^:])\/\/.*$/gm, "$1");
  const bad: string[] = [];
  for (const m of code.matchAll(candidate)) {
    const prefix = m[1] ?? "";
    const value = m[2] ?? "";
    if (value.startsWith("[")) {
      bad.push(m[0]);
    } else if (prefix === "text") {
      const name = value.split("/")[0] ?? "";
      if (!TEXT_KEYWORDS.has(name) && !has(["text", "color"], name))
        bad.push(m[0]);
    } else if (/^\d/.test(value)) {
      const namespaces = SIZING[prefix] ?? ["spacing"];
      const fraction = /^\d+\/\d+$/.test(value) && FRACTIONS.has(prefix);
      if (!fraction && !has(namespaces, value)) bad.push(m[0]);
    } else if (NAMED_CHECKED.has(prefix)) {
      const namespaces = SIZING[prefix] ?? [];
      if (!SIZE_KEYWORDS.has(value) && !has(namespaces, value)) bad.push(m[0]);
    }
  }
  return bad;
}

const keys = themeKeys(themeCss);

describe("utility guard", () => {
  it("reads the token scale from the Tailwind theme", () => {
    expect(keys.get("spacing")?.has("9")).toBe(true);
    expect(keys.get("text")?.has("wordmark")).toBe(true);
    expect(keys.get("text")?.has("wordmark--letter-spacing")).toBe(false);
    expect(keys.get("height")?.has("bar")).toBe(true);
    expect(keys.get("width")?.has("skeleton-s")).toBe(true);
    expect(keys.get("font-weight")?.has("semibold")).toBe(true);
  });
  it("checks a named min or max size against the namespace Tailwind reads for it", () => {
    expect(keys.get("container")).toBeUndefined();
    expect(keys.get("max-width")).toBeUndefined();
    expect(offToken('className="max-w-content"', keys)).toEqual([
      "max-w-content",
    ]);
    // max-w reads --max-width, --spacing and --container, never --width.
    expect(offToken('className="max-w-skeleton-l"', keys)).toEqual([
      "max-w-skeleton-l",
    ]);
    expect(offToken('className="min-w-thread-list max-h-x"', keys)).toEqual([
      "min-w-thread-list",
      "max-h-x",
    ]);
    // min-h and max-h also read --height.
    expect(
      offToken('className="min-h-bar max-h-bar max-w-full min-w-max"', keys),
    ).toEqual([]);
    const own = themeKeys(
      "@theme { --max-width-content: 1px; --container-wide: 2px; }",
    );
    expect(offToken('className="max-w-content min-w-wide"', own)).toEqual([]);
  });
  it("accepts utilities that resolve to a token", () => {
    const ok = [
      'className="p-9 gap-16 md:px-22 -mt-2 p-4! h-bar min-h-bar w-skeleton-l"',
      'cx("text-wordmark", on && "hover:text-primary/50 text-muted text-center")',
      '"w-full w-1/2 basis-1/3 mx-auto size-6 inset-0 top-1/2"',
      "// gap-15 in a comment",
      '"T-12 · ws-9 · src/text-files/a.ts"',
    ];
    for (const text of ok) expect(offToken(text, keys), text).toEqual([]);
  });
  it("rejects values outside the token scale", () => {
    const bad = [
      "gap-15",
      "md:px-11",
      "-mt-1",
      "p-[13px]",
      "h-52",
      "w-[70px]",
      "max-w-70",
      "size-[3px]",
      "text-13",
      "text-[13px]",
      "text-primay",
      "p-1/2",
    ];
    for (const word of bad)
      expect(offToken(`className="${word}"`, keys), word).toEqual([word]);
  });
  it("finds no off-token utility in the sources", () => {
    const files = Object.entries(sources).filter(
      ([p]) => !p.endsWith(".test.ts") && !p.endsWith(".gen.ts"),
    );
    expect(files.length).toBeGreaterThan(3);
    for (const [path, text] of files)
      expect(offToken(text, keys), path).toEqual([]);
  });
});
