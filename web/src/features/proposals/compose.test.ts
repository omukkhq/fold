import { describe, expect, it } from "vitest";
import { MAX_TEXT_CHARS } from "@/format/text";
import { commentSchema, requestChangesSchema } from "./compose";

describe("review bodies", () => {
  it("need text and count characters as the daemon does, after trimming", () => {
    expect(commentSchema.safeParse({ body: "  " }).success).toBe(false);
    expect(requestChangesSchema.safeParse({ body: "Split it." }).success).toBe(
      true,
    );
    const atLimit = "😀".repeat(MAX_TEXT_CHARS);
    expect(commentSchema.safeParse({ body: ` ${atLimit} ` }).success).toBe(
      true,
    );
    expect(commentSchema.safeParse({ body: `${atLimit}a` }).success).toBe(
      false,
    );
  });
});
