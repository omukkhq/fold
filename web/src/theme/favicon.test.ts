import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import html from "../../index.html?raw";
import svg from "../../public/favicon.svg?raw";
import tokensCss from "../styles/tokens.css?raw";

function themeBlock(selector: string): Map<string, string> {
  const blocks = [...tokensCss.matchAll(/([^{}]+)\{([^{}]*)\}/g)];
  const body = blocks.find(
    (m) => (m[1] ?? "").trim() === selector && m[2]?.includes("color-scheme"),
  )?.[2];
  if (body === undefined) throw new Error(`missing block ${selector}`);
  return new Map(
    [...body.matchAll(/--([\w-]+)\s*:\s*([^;]+);/g)].map((m) => [
      m[1] ?? "",
      (m[2] ?? "").trim(),
    ]),
  );
}

function fills(css: string): Record<string, string> {
  return Object.fromEntries(
    [...css.matchAll(/\.([\w-]+)\s*\{\s*fill:\s*([^;]+);/g)].map((m) => [
      m[1] ?? "",
      (m[2] ?? "").trim(),
    ]),
  );
}

/** Width and height from a PNG's IHDR chunk. */
function pngSize(name: string): [number, number] {
  const bytes = readFileSync(new URL(`../../public/${name}`, import.meta.url));
  expect(bytes.subarray(1, 4).toString("latin1")).toBe("PNG");
  return [bytes.readUInt32BE(16), bytes.readUInt32BE(20)];
}

describe("favicon", () => {
  const style = /<style>([\s\S]*)<\/style>/.exec(svg)?.[1] ?? "";
  const [lightCss = "", darkCss = ""] = style.split(
    "@media (prefers-color-scheme: dark)",
  );

  it("draws the f in the text colour and the dot in the primary colour of the light theme", () => {
    const light = themeBlock(":root");
    expect(fills(lightCss)).toEqual({
      f: light.get("text"),
      dot: light.get("primary"),
    });
  });

  it("switches to the dark theme's colours when the system is dark", () => {
    const dark = themeBlock(':root[data-theme="dark"]');
    expect(fills(darkCss)).toEqual({
      f: dark.get("text"),
      dot: dark.get("primary"),
    });
  });

  it("draws outlines, not text, since a favicon cannot load the bundled font", () => {
    expect(svg).not.toMatch(/<text/);
    expect(svg).toMatch(/<path\s[^>]*class="f"/);
    expect(svg).toMatch(/<path\s[^>]*class="dot"/);
  });

  it("is linked from index.html with the PNG fallback before the SVG, since browsers take the last icon they support, and an apple-touch icon", () => {
    const links = [...html.matchAll(/<link rel="([^"]+)" href="([^"]+)"/g)].map(
      (m) => `${m[1] ?? ""} ${m[2] ?? ""}`,
    );
    expect(links).toEqual([
      "icon /favicon-32.png",
      "icon /favicon.svg",
      "apple-touch-icon /apple-touch-icon.png",
    ]);
  });

  it("ships the PNGs at the sizes the links name", () => {
    expect(pngSize("favicon-32.png")).toEqual([32, 32]);
    expect(pngSize("apple-touch-icon.png")).toEqual([180, 180]);
  });
});
