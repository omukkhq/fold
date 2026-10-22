import { describe, expect, it } from "vitest";
import type { MessageLines } from "@/api/messages";
import { cutNote, lineRefHref, lineRefText, quotedLines } from "./lineRef";

function lines(fields: Partial<MessageLines> = {}): MessageLines {
  return {
    version: 2,
    path: "web/src/board.ts",
    side: "new",
    start_line: 7,
    end_line: 9,
    excerpt: "a\n\nc",
    excerpt_cut: false,
    ...fields,
  };
}

describe("the lines a question is about", () => {
  it("names the file, the version, the lines and the side", () => {
    expect(lineRefText(lines())).toBe(
      "web/src/board.ts · v2 · lines 7–9 · new side",
    );
    expect(
      lineRefText(lines({ start_line: 3, end_line: 3, side: "old" })),
    ).toBe("web/src/board.ts · v2 · line 3 · old side");
  });

  it("numbers the excerpt's lines from the first line of the run, empty lines included", () => {
    expect(quotedLines(lines())).toEqual([
      { number: 7, text: "a" },
      { number: 8, text: "" },
      { number: 9, text: "c" },
    ]);
  });

  it("says how much of a cut excerpt it shows", () => {
    expect(cutNote(lines())).toBeNull();
    expect(
      cutNote(
        lines({
          start_line: 1,
          end_line: 150,
          excerpt: "a\nb",
          excerpt_cut: true,
        }),
      ),
    ).toBe("The first 2 of 150 lines.");
  });

  it("links to the proposal's diff at the version and the file", () => {
    expect(lineRefHref("demo", "P-4", lines())).toBe(
      "/p/demo/proposals/P-4?version=2&path=web%2Fsrc%2Fboard.ts",
    );
  });
});
