import { describe, expect, it } from "vitest";
import { createSseParser, type SseFrame } from "./sse-parse";

const wire =
  ': keep-alive\r\nid: 7\r\nevent: message\r\ndata: {"a":1}\r\n\r\ndata: one\ndata: two\n\n';

const expected: SseFrame[] = [
  { kind: "comment", text: "keep-alive" },
  { kind: "message", id: "7", event: "message", data: '{"a":1}' },
  { kind: "message", id: null, event: null, data: "one\ntwo" },
];

function parseSplit(cuts: number[]): SseFrame[] {
  const parser = createSseParser();
  const frames: SseFrame[] = [];
  let from = 0;
  for (const cut of [...cuts, wire.length]) {
    frames.push(...parser.push(wire.slice(from, cut)));
    from = cut;
  }
  return frames;
}

describe("createSseParser", () => {
  it("parses a whole buffer", () => {
    expect(parseSplit([])).toEqual(expected);
  });

  it("parses when split at every position", () => {
    for (let cut = 1; cut < wire.length; cut += 1) {
      expect(parseSplit([cut])).toEqual(expected);
    }
  });

  it("parses when fed one character at a time", () => {
    expect(
      parseSplit([...wire.slice(1).split("").keys()].map((i) => i + 1)),
    ).toEqual(expected);
  });

  it("splits inside a field name, a CRLF and between frames", () => {
    const at = (needle: string, offset: number) =>
      wire.indexOf(needle) + offset;
    expect(parseSplit([at("event", 3)])).toEqual(expected);
    expect(parseSplit([at("\r\n\r\n", 1)])).toEqual(expected);
    expect(parseSplit([at("data: one", 0)])).toEqual(expected);
  });

  it("accepts bare CR and bare LF line endings", () => {
    const parser = createSseParser();
    expect(parser.push("data: a\r\rdata: b\n\ndata: c\r")).toEqual([
      { kind: "message", id: null, event: null, data: "a" },
      { kind: "message", id: null, event: null, data: "b" },
    ]);
    expect(parser.push("\n\n")).toEqual([
      { kind: "message", id: null, event: null, data: "c" },
    ]);
  });

  it("handles a field with no colon, no space, and ignores unknown fields", () => {
    const parser = createSseParser();
    expect(parser.push("data\ndata:x\nretry: 5\n\n")).toEqual([
      { kind: "message", id: null, event: null, data: "\nx" },
    ]);
  });

  it("drops a block with no data and resets its fields", () => {
    const parser = createSseParser();
    expect(parser.push("id: 1\nevent: x\n\ndata: y\n\n")).toEqual([
      { kind: "message", id: null, event: null, data: "y" },
    ]);
  });
});
