import { describe, expect, it } from "vitest";
import {
  MAX_BODY,
  feedbackSchema,
  replySchema,
  insertMention,
} from "./compose";

function messages(result: {
  success: boolean;
  error?: { issues: { path: PropertyKey[]; message: string }[] };
}) {
  return (result.error?.issues ?? []).map((i) => [i.path.join("."), i.message]);
}

describe("replySchema", () => {
  it("refuses an empty or blank body", () => {
    for (const body of ["", "  \n "]) {
      expect(messages(replySchema.safeParse({ body }))).toEqual([
        ["body", "Write a message first."],
      ]);
    }
  });
  it("refuses a body over the limit and accepts one at it", () => {
    expect(
      messages(
        replySchema.safeParse({ body: "a".repeat(MAX_BODY + 1) }),
      )[0]?.[1],
    ).toMatch(/at most/);
    expect(replySchema.safeParse({ body: "a".repeat(MAX_BODY) }).success).toBe(
      true,
    );
  });
});

describe("the body limit", () => {
  it("is the daemon's, counted in characters rather than UTF-16 units", () => {
    expect(MAX_BODY).toBe(20_000);
    // 10 001 emoji are 20 002 UTF-16 units but 10 001 characters.
    expect(replySchema.safeParse({ body: "😀".repeat(10_001) }).success).toBe(
      true,
    );
    expect(replySchema.safeParse({ body: "😀".repeat(20_001) }).success).toBe(
      false,
    );
  });
});

describe("feedbackSchema", () => {
  it("accepts one task id and a body", () => {
    expect(
      feedbackSchema.safeParse({ task: " T-12 ", body: "Hi" }).success,
    ).toBe(true);
  });
  it("refuses anything but one task id", () => {
    for (const task of ["", "P-3", "T-12 and T-13", "12", "T-0"]) {
      expect(messages(feedbackSchema.safeParse({ task, body: "Hi" }))).toEqual([
        ["task", "Enter a task id such as T-12."],
      ]);
    }
  });
  it("reports both fields", () => {
    expect(
      messages(feedbackSchema.safeParse({ task: "x", body: "" })),
    ).toHaveLength(2);
  });
});

describe("insertMention", () => {
  it("writes @ at the caret, after a space when it follows a word", () => {
    expect(insertMention("", 0)).toEqual({ text: "@", caret: 1 });
    expect(insertMention("ask ", 4)).toEqual({ text: "ask @", caret: 5 });
    expect(insertMention("ask", 3)).toEqual({ text: "ask @", caret: 5 });
    expect(insertMention("ab cd", 2)).toEqual({ text: "ab @ cd", caret: 4 });
  });
  it("keeps the caret inside the text", () => {
    expect(insertMention("hi", 9)).toEqual({ text: "hi @", caret: 4 });
    expect(insertMention("hi", -1)).toEqual({ text: "@hi", caret: 1 });
  });
});
