import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import html from "../../index.html?raw";

function publicFile(name: string): Buffer {
  return readFileSync(new URL(`../../public/${name}`, import.meta.url));
}

/** Width and height from a PNG's IHDR chunk. */
function pngSize(name: string): [number, number] {
  const bytes = publicFile(name);
  expect(bytes.subarray(1, 4).toString("latin1")).toBe("PNG");
  return [bytes.readUInt32BE(16), bytes.readUInt32BE(20)];
}

/** The square sizes in an ICO's directory, where a stored 0 means 256. */
function icoSizes(name: string): number[] {
  const bytes = publicFile(name);
  expect(bytes.readUInt16LE(2)).toBe(1);
  return Array.from(
    { length: bytes.readUInt16LE(4) },
    (_, i) => bytes[6 + 16 * i] || 256,
  );
}

describe("favicon", () => {
  it("is linked from index.html with the ICO before the PNG, since the browsers that ignore sizes take the last icon they support, and an apple-touch icon", () => {
    const links = [...html.matchAll(/<link rel="([^"]+)" href="([^"]+)"/g)].map(
      (m) => `${m[1] ?? ""} ${m[2] ?? ""}`,
    );
    expect(links).toEqual([
      "icon /favicon.ico",
      "icon /favicon.png",
      "apple-touch-icon /apple-touch-icon.png",
    ]);
  });

  it("ships the icons at the sizes browsers ask for", () => {
    expect(icoSizes("favicon.ico")).toEqual([16, 32, 48]);
    expect(pngSize("favicon.png")).toEqual([512, 512]);
    expect(pngSize("apple-touch-icon.png")).toEqual([180, 180]);
  });

  it("names every size each icon holds, since the browsers that read sizes choose by them", () => {
    const sizes = Object.fromEntries(
      [
        ...html.matchAll(
          /<link rel="icon" href="\/([^"]+)"[^>]*sizes="([^"]+)"/g,
        ),
      ].map((m) => [m[1] ?? "", m[2] ?? ""]),
    );
    const square = (n: number) => `${String(n)}x${String(n)}`;
    expect(sizes).toEqual({
      "favicon.ico": icoSizes("favicon.ico").map(square).join(" "),
      "favicon.png": square(pngSize("favicon.png")[0]),
    });
  });
});
