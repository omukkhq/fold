import { describe, expect, it } from "vitest";
import {
  checklistItemSchema,
  dependencySchema,
  descriptionSchema,
  feedbackSchema,
  humanReviewSchema,
  labelSchema,
  renameItemSchema,
  titleSchema,
} from "./schemas";

function firstMessage(result: {
  success: boolean;
  error?: { issues: { message: string }[] };
}): string | undefined {
  return result.error?.issues[0]?.message;
}

describe("titleSchema", () => {
  it("trims a title and refuses an empty, multi-line or long one", () => {
    expect(titleSchema.parse({ title: "  Sign in  " })).toEqual({
      title: "Sign in",
    });
    expect(firstMessage(titleSchema.safeParse({ title: "  " }))).toBe(
      "Write a title.",
    );
    expect(firstMessage(titleSchema.safeParse({ title: "a\nb" }))).toBe(
      "A title is one line.",
    );
    expect(titleSchema.safeParse({ title: "x".repeat(201) }).success).toBe(
      false,
    );
  });
});

describe("descriptionSchema", () => {
  it("takes an empty description and refuses a very long one", () => {
    expect(descriptionSchema.safeParse({ description: "" }).success).toBe(true);
    expect(
      descriptionSchema.safeParse({ description: "x".repeat(20_001) }).success,
    ).toBe(false);
  });
});

describe("labelSchema", () => {
  it("takes a name and one of the eight colours", () => {
    expect(labelSchema.parse({ name: " ui ", color: "sage" })).toEqual({
      name: "ui",
      color: "sage",
    });
    expect(
      firstMessage(labelSchema.safeParse({ name: "ui", color: "red" })),
    ).toBe("Choose one of the eight colours.");
    expect(
      firstMessage(labelSchema.safeParse({ name: "", color: "sage" })),
    ).toBe("Write a label name.");
  });
});

describe("checklistItemSchema", () => {
  it("takes a plain step's text or a linked task's id", () => {
    expect(checklistItemSchema.parse({ kind: "step", text: " Copy " })).toEqual(
      { kind: "step", text: "Copy" },
    );
    expect(
      checklistItemSchema.parse({ kind: "linked", text: " T-13 " }),
    ).toEqual({ kind: "linked", text: "T-13" });
  });

  it("refuses an empty step and a linked item that is not a task id", () => {
    expect(
      firstMessage(checklistItemSchema.safeParse({ kind: "step", text: "" })),
    ).toBe("Write the step.");
    expect(
      firstMessage(
        checklistItemSchema.safeParse({ kind: "linked", text: "login" }),
      ),
    ).toBe("Write a task id such as T-12.");
    expect(renameItemSchema.safeParse({ text: "" }).success).toBe(false);
  });
});

describe("dependencySchema", () => {
  it("takes another task's id and refuses the task itself", () => {
    const schema = dependencySchema("T-2");
    expect(schema.parse({ task: "T-5" })).toEqual({ task: "T-5" });
    expect(firstMessage(schema.safeParse({ task: "T-2" }))).toBe(
      "A task cannot depend on itself.",
    );
    expect(firstMessage(schema.safeParse({ task: "T-0" }))).toBe(
      "Write a task id such as T-12.",
    );
  });
});

describe("humanReviewSchema", () => {
  it("makes the reviewer optional and the reason required", () => {
    expect(humanReviewSchema.parse({ reviewer: " ", reason: "auth" })).toEqual({
      reviewer: null,
      reason: "auth",
    });
    expect(
      humanReviewSchema.parse({ reviewer: "tanim", reason: "auth" }).reviewer,
    ).toBe("tanim");
    expect(
      firstMessage(humanReviewSchema.safeParse({ reviewer: "", reason: "" })),
    ).toBe("Say why the task needs a human review.");
  });
});

describe("feedbackSchema", () => {
  it("needs a reader and a body", () => {
    expect(feedbackSchema.parse({ to: "holder", body: " Hi " })).toEqual({
      to: "holder",
      body: "Hi",
    });
    expect(firstMessage(feedbackSchema.safeParse({ to: "", body: "x" }))).toBe(
      "Choose who reads the feedback.",
    );
    expect(
      firstMessage(feedbackSchema.safeParse({ to: "holder", body: " " })),
    ).toBe("Write the feedback.");
  });
});
