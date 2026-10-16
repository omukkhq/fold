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
