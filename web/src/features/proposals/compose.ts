import { z } from "zod";

import { MAX_TEXT_CHARS, charCount } from "@/format/text";

/** The daemon trims a review or comment body, then counts it as `charCount` does. */
function body(empty: string) {
  return z
    .string()
    .refine((value) => value.trim().length > 0, empty)
    .refine(
      (value) => charCount(value.trim()) <= MAX_TEXT_CHARS,
      `A review holds at most ${String(MAX_TEXT_CHARS)} characters.`,
    );
}

export const commentSchema = z.object({ body: body("Write a comment first.") });

/** Request changes needs a body: the author reads it to know what to change. */
export const requestChangesSchema = z.object({
  body: body("Say what needs to change."),
});

export type CommentValues = z.infer<typeof commentSchema>;
export type RequestChangesValues = z.infer<typeof requestChangesSchema>;
