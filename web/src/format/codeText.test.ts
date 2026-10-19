import { describe, expect, it } from "vitest";
import { codeData, codeParts } from "./codeText";

describe("codeParts", () => {
  it("keeps a sentence without a mark as one string", () => {
    expect(codeParts("Nothing to run.")).toEqual(["Nothing to run."]);
  });
  it("splits plain words and commands in order", () => {
    expect(codeParts("Run `fld doctor` to see why, or `fld init`.")).toEqual([
      "Run ",
      { code: "fld doctor" },
      " to see why, or ",
      { code: "fld init" },
      ".",
    ]);
  });
  it("starts and ends with a command", () => {
    expect(codeParts("`a` and `b`")).toEqual([
      { code: "a" },
      " and ",
      { code: "b" },
    ]);
  });
  it("keeps adjacent commands apart", () => {
    expect(codeParts("`a``b`")).toEqual([{ code: "a" }, { code: "b" }]);
  });
  it("keeps the spaces and hyphens of a command", () => {
    expect(codeParts("run `git restore --staged` on it")).toEqual([
      "run ",
      { code: "git restore --staged" },
      " on it",
    ]);
  });
  it("leaves a backtick without its pair as a plain character", () => {
    expect(codeParts("a ` b")).toEqual(["a ` b"]);
    expect(codeParts("`a` then `b")).toEqual([{ code: "a" }, " then `b"]);
  });
  it("drops an empty pair and gives nothing for an empty text", () => {
    expect(codeParts("a `` b")).toEqual(["a  b"]);
    expect(codeParts("")).toEqual([]);
  });
  it("reads an escaped backtick or backslash as a plain character", () => {
    expect(codeParts("a \\` b `c` \\\\ d \\e")).toEqual([
      "a ` b ",
      { code: "c" },
      " \\ d \\e",
    ]);
  });
});

describe("codeData", () => {
  it("keeps a backtick in data from pairing with a command's marks", () => {
    const path = "a`b.md";
    expect(
      codeParts(`changes to ${codeData(path)}; run \`fld record\``),
    ).toEqual(["changes to a`b.md; run ", { code: "fld record" }]);
  });
  it("gives back data with backslashes and backticks as it is", () => {
    const path = "dir\\`x`\\";
    expect(codeParts(codeData(path))).toEqual([path]);
  });
  it("leaves plain data as it is", () => {
    expect(codeData("src/app.ts")).toBe("src/app.ts");
  });
});
