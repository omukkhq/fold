import { z } from "zod";
import { MAX_TEXT_CHARS, charCount } from "@/format/text";

function text(empty: string) {
  return z
    .string()
    .refine((value) => value.trim().length > 0, empty)
    .refine(
      (value) => charCount(value) <= MAX_TEXT_CHARS,
      `Write at most ${String(MAX_TEXT_CHARS)} characters.`,
    );
}

export const newRequestSchema = z.object({
  body: text("Say what you need first."),
});
export const changesSchema = z.object({
  comment: text("Say what needs to change."),
});
export const closeSchema = z.object({
  reason: text("Say why you close the request."),
});
export const messageSchema = z.object({ body: text("Write a message first.") });

export type NewRequestValues = z.infer<typeof newRequestSchema>;
export type ChangesValues = z.infer<typeof changesSchema>;
export type CloseValues = z.infer<typeof closeSchema>;
export type MessageValues = z.infer<typeof messageSchema>;
