import { z } from "zod";
import { parseId } from "@/format/ids";
import { MAX_TEXT_CHARS, charCount } from "@/format/text";

export const MAX_BODY = MAX_TEXT_CHARS;

const body = z
  .string()
  .refine((value) => value.trim().length > 0, "Write a message first.")
  .refine(
    (value) => charCount(value.trim()) <= MAX_BODY,
    `A message holds at most ${String(MAX_BODY)} characters.`,
  );

const task = z
  .string()
  .refine(
    (value) => parseId(value.trim())?.kind === "task",
    "Enter a task id such as T-12.",
  );

export const replySchema = z.object({ body });

export const feedbackSchema = z.object({ task, body });

export type ReplyValues = z.infer<typeof replySchema>;
export type FeedbackValues = z.infer<typeof feedbackSchema>;

/** The text with `@` written at the caret, after a space when the caret follows a word, so the box suggests people and files at once; and the caret behind the `@`. */
export function insertMention(
  text: string,
  caret: number,
): { text: string; caret: number } {
  const at = Math.min(Math.max(caret, 0), text.length);
  const before = text.slice(0, at);
  const mark = before === "" || /\s$/.test(before) ? "@" : " @";
  return {
    text: `${before}${mark}${text.slice(at)}`,
    caret: at + mark.length,
  };
}
