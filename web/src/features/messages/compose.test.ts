import { describe, expect, it } from "vitest";
import { MAX_BODY, feedbackSchema, replySchema } from "./compose";

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
