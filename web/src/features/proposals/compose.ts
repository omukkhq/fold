import { z } from "zod";

export const MAX_BODY = 8000;

function body(empty: string) {
  return z
    .string()
    .refine((value) => value.trim().length > 0, empty)
    .refine(
      (value) => value.trim().length <= MAX_BODY,
      `A review holds at most ${String(MAX_BODY)} characters.`,
    );
}

export const commentSchema = z.object({ body: body("Write a comment first.") });

/** Request changes needs a body: the author reads it to know what to change. */
export const requestChangesSchema = z.object({
  body: body("Say what needs to change."),
});

export type CommentValues = z.infer<typeof commentSchema>;
export type RequestChangesValues = z.infer<typeof requestChangesSchema>;
