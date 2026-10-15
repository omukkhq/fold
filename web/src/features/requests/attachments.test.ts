import { describe, expect, it } from "vitest";
import {
  MAX_FILE_BYTES,
  addFiles,
  blockingProblem,
  fileProblem,
  refusedText,
  validName,
} from "./attachments";
import {
  changesSchema,
  closeSchema,
  messageSchema,
  newRequestSchema,
} from "./compose";

const file = (name: string, size = 10) => ({ name, size });

describe("validName", () => {
  it("follows the daemon's name rule", () => {
    expect(validName("report-mockup.png")).toBe(true);
    expect(validName("")).toBe(false);
    expect(validName("a/b.png")).toBe(false);
    expect(validName("a..b")).toBe(false);
    expect(validName("tab\there")).toBe(false);
    expect(validName("x\u0085y")).toBe(false);
    expect(validName("é".repeat(128))).toBe(false);
    expect(validName("e".repeat(255))).toBe(true);
  });
});

describe("checks before upload", () => {
  it("names a file over 10 MiB and a bad name", () => {
    expect(fileProblem(file("ok.csv"))).toBeNull();
    expect(fileProblem(file("big.png", 14.2 * 1024 * 1024))).toBe(
      "big.png is 14.2 MiB. Each file can be up to 10 MiB. Remove it or attach a smaller file.",
    );
    expect(fileProblem(file("edge.bin", MAX_FILE_BYTES))).toBeNull();
    expect(fileProblem(file(""))).toMatch(/Rename the file/);
  });

  it("adds files up to ten and counts the rest", () => {
    const nine = Array.from({ length: 9 }, (_, i) => file(`f${String(i)}`));
    const next = addFiles(nine, [file("a"), file("b"), file("c")]);
    expect(next.files).toHaveLength(10);
    expect(next.refused).toBe(2);
    expect(addFiles([], [file("a")]).refused).toBe(0);
    expect(refusedText(0)).toBeNull();
    expect(refusedText(1)).toBe(
      "1 file was left out: a request holds at most 10 files.",
    );
    expect(refusedText(2)).toMatch(/^2 files were left out/);
  });

  it("stops the upload at the first problem", () => {
    expect(blockingProblem([file("a")])).toBeNull();
    expect(blockingProblem([file("a"), file("b", MAX_FILE_BYTES + 1)])).toMatch(
      /^b is/,
    );
    expect(
      blockingProblem(
        Array.from({ length: 11 }, (_, i) => file(`f${String(i)}`)),
      ),
    ).toBe("A request holds at most 10 files.");
  });
});

describe("form schemas", () => {
  it("needs text that is not blank and not too long", () => {
    expect(newRequestSchema.safeParse({ body: "Need X" }).success).toBe(true);
    const blank = newRequestSchema.safeParse({ body: "  " });
    expect(blank.success ? null : blank.error.issues[0]?.message).toBe(
      "Say what you need first.",
    );
    const long = changesSchema.safeParse({ comment: "x".repeat(20_001) });
    expect(long.success ? null : long.error.issues[0]?.message).toMatch(
      /at most 20000/,
    );
    expect(closeSchema.safeParse({ reason: "" }).success).toBe(false);
    expect(messageSchema.safeParse({ body: "hi" }).success).toBe(true);
  });
});
