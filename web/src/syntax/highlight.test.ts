import { afterEach, describe, expect, it, vi } from "vitest";
import {
  MAX_CHARS,
  MAX_LINE_LENGTH,
  MAX_LINES,
  highlightSegments,
  withinLimits,
  type SyntaxLine,
} from "./highlight";
import { GRAMMARS } from "./shiki";
import { syntaxColor } from "./theme";

afterEach(() => {
  vi.restoreAllMocks();
});

function colorOf(line: SyntaxLine | undefined, content: string) {
  return line?.find((token) => token.content.trim() === content)?.color;
}

describe("highlightSegments", () => {
  it("colours tokens with the syntax variables and leaves plain text uncoloured", async () => {
    const result = await highlightSegments("fence:typescript", [
      ["let answer = 42; // why"],
    ]);
    const line = result?.[0]?.[0];
    expect(colorOf(line, "let")).toBe(syntaxColor("keyword"));
    expect(colorOf(line, "42")).toBe(syntaxColor("number"));
    expect(colorOf(line, "// why")).toBe(syntaxColor("comment"));
    expect(colorOf(line, "answer")).toBeNull();
    expect(line?.map((token) => token.content).join("")).toBe(
      "let answer = 42; // why",
    );
  });

  it("carries the grammar state across chunks, so a comment that spans one stays a comment", async () => {
    const result = await highlightSegments(
      "fence:typescript",
      [["/* one", "two", "three */", "let x"]],
      { chunkLines: 1 },
    );
    const lines = result?.[0] ?? [];
    expect(lines).toHaveLength(4);
    expect(colorOf(lines[1], "two")).toBe(syntaxColor("comment"));
    expect(colorOf(lines[2], "three */")).toBe(syntaxColor("comment"));
    expect(colorOf(lines[3], "let")).toBe(syntaxColor("keyword"));
  });

  it("starts each segment from a fresh state", async () => {
    const result = await highlightSegments("fence:typescript", [
      ["/* open"],
      ["let x"],
    ]);
    expect(colorOf(result?.[1]?.[0], "let")).toBe(syntaxColor("keyword"));
  });

  it("gives every input line a line of tokens, empty lines included", async () => {
    const result = await highlightSegments("fence:rust", [
      ["fn main() {", "", "}"],
    ]);
    expect(result?.[0]?.map((line) => line.length > 0)).toEqual([
      true,
      false,
      true,
    ]);
  });

  it("leaves a line past the length limit plain and highlights the next one", async () => {
    const long = `let x = "${"a".repeat(MAX_LINE_LENGTH)}";`;
    const result = await highlightSegments("fence:typescript", [
      [long, "let y"],
    ]);
    const lines = result?.[0] ?? [];
    expect(lines[0]?.every((token) => token.color === null)).toBe(true);
    expect(colorOf(lines[1], "let")).toBe(syntaxColor("keyword"));
  });

  it("is null without a language or past the limits", async () => {
    expect(await highlightSegments(null, [["let x"]])).toBeNull();
    const many = Array.from({ length: MAX_LINES + 1 }, () => "x");
    expect(await highlightSegments("fence:typescript", [many])).toBeNull();
  });

  it("is null once the work is stopped", async () => {
    const controller = new AbortController();
    controller.abort();
    expect(
      await highlightSegments("fence:typescript", [["let x"]], {
        signal: controller.signal,
      }),
    ).toBeNull();
  });

  it("is null when the work stops while it runs", async () => {
    const controller = new AbortController();
    const pending = highlightSegments(
      "fence:typescript",
      [["let x", "let y"]],
      {
        signal: controller.signal,
        chunkLines: 1,
        yieldAfterMs: 0,
      },
    );
    setTimeout(() => {
      controller.abort();
    }, 0);
    expect(await pending).toBeNull();
  });

  it("is null when a grammar fails to load, and loads it on the next try", async () => {
    vi.spyOn(GRAMMARS, "zig").mockRejectedValueOnce(new Error("offline"));
    expect(await highlightSegments("fence:zig", [["const x = 1;"]])).toBeNull();
    const result = await highlightSegments("fence:zig", [["const x = 1;"]]);
    expect(colorOf(result?.[0]?.[0], "const")).toBe(syntaxColor("keyword"));
  });
});

describe("withinLimits", () => {
  it("counts lines and characters over all segments", () => {
    expect(withinLimits([["a"], ["b"]])).toBe(true);
    expect(withinLimits([["a".repeat(MAX_CHARS)]])).toBe(false);
    const half = Array.from({ length: MAX_LINES / 2 + 1 }, () => "x");
    expect(withinLimits([half, half])).toBe(false);
  });
});
