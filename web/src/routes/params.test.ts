import { describe, expect, it } from "vitest";
import { parseProposalParam, parseSlug, parseTaskParam } from "./params";

describe("parseTaskParam", () => {
  it("reads a task id", () => {
    expect(parseTaskParam("T-12")).toBe(12);
  });
  it.each([
    "T-0",
    "t-12",
    "T-12abc",
    "",
    "T-",
    "T-012",
    "P-3",
    "T-4294967296",
    "T--1",
  ])("rejects %j", (raw) => {
    expect(parseTaskParam(raw)).toBeNull();
  });
  it("accepts the largest id", () => {
    expect(parseTaskParam("T-4294967295")).toBe(4_294_967_295);
  });
});

describe("parseProposalParam", () => {
  it("reads a proposal id", () => {
    expect(parseProposalParam("P-3")).toBe(3);
  });
  it.each(["P-0", "p-3", "P-3abc", "", "T-3"])("rejects %j", (raw) => {
    expect(parseProposalParam(raw)).toBeNull();
  });
});

describe("parseSlug", () => {
  it.each(["demo", "omukk-site", "a1"])("accepts %j", (raw) => {
    expect(parseSlug(raw)).toBe(raw);
  });
  it.each(["", "Demo", "-demo", "de mo", "de/mo", "a".repeat(65)])(
    "rejects %j",
    (raw) => {
      expect(parseSlug(raw)).toBeNull();
    },
  );
});
