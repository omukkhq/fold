import { z } from "zod";
import { MAX_TEXT_CHARS, charCount } from "@/format/text";
import { LABEL_NAMES } from "@/components/label/labelColors";
import { parseTaskParam } from "@/routes/params";

/** The daemon's limit on one text body, counted in characters as it counts them. */
function textLimit(noun: string): [(value: string) => boolean, string] {
  return [
    (value) => charCount(value) <= MAX_TEXT_CHARS,
    `Keep the ${noun} to ${String(MAX_TEXT_CHARS)} characters.`,
  ];
}

const taskId = z
  .string()
  .trim()
  .refine((value) => parseTaskParam(value) !== null, {
    message: "Write a task id such as T-12.",
  });

export const titleSchema = z.object({
  title: z
    .string()
    .trim()
    .min(1, "Write a title.")
    .max(200, "Keep the title to 200 characters.")
    .refine((value) => !value.includes("\n"), "A title is one line."),
});
export type TitleForm = z.infer<typeof titleSchema>;

export const newTaskSchema = z.object({
  title: titleSchema.shape.title,
  description: z.string().refine(...textLimit("description")),
  kind: z.enum(["proposal", "no_proposal", "human"]),
  /** An area name, or empty to let the main agent choose. */
  area: z.string(),
  labels: z.array(z.string()),
});
export type NewTaskForm = z.infer<typeof newTaskSchema>;

export const descriptionSchema = z.object({
  description: z.string().refine(...textLimit("description")),
});
export type DescriptionForm = z.infer<typeof descriptionSchema>;

export const labelSchema = z.object({
  name: z
    .string()
    .trim()
    .min(1, "Write a label name.")
    .max(40, "Keep the name to 40 characters.")
    .refine(
      (value) => value !== "." && value !== "..",
      "A label name cannot be . or ..",
    ),
  color: z.enum(LABEL_NAMES, { message: "Choose one of the eight colours." }),
});
export type LabelForm = z.infer<typeof labelSchema>;

const stepText = z
  .string()
  .trim()
  .min(1, "Write the step.")
  .max(200, "Keep the step to 200 characters.");

export const checklistItemSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("step"), text: stepText }),
  z.object({ kind: z.literal("linked"), text: taskId }),
]);
export type ChecklistItemForm = z.infer<typeof checklistItemSchema>;

export const renameItemSchema = z.object({ text: stepText });
export type RenameItemForm = z.infer<typeof renameItemSchema>;

/** The task cannot need itself; the daemon refuses cycles, so only it can name one. */
export function dependencySchema(self: string) {
  return z.object({
    task: taskId.refine((value) => value !== self, {
      message: "A task cannot depend on itself.",
    }),
  });
}
export type DependencyForm = z.infer<ReturnType<typeof dependencySchema>>;

export const humanReviewSchema = z.object({
  reviewer: z
    .string()
    .trim()
    .max(64, "Keep the name to 64 characters.")
    .transform((value) => (value === "" ? null : value)),
  reason: z
    .string()
    .trim()
    .min(1, "Say why the task needs a human review.")
    .max(500, "Keep the reason to 500 characters."),
});
export type HumanReviewInput = z.input<typeof humanReviewSchema>;
export type HumanReviewForm = z.output<typeof humanReviewSchema>;

export const blockerSchema = z.object({
  reason: z
    .string()
    .trim()
    .min(1, "Say why the task waits for a person.")
    .refine(
      (value) => charCount(value) <= 500,
      "Keep the reason to 500 characters.",
    ),
});
export type BlockerForm = z.infer<typeof blockerSchema>;

export const sendBackSchema = z.object({
  note: z
    .string()
    .trim()
    .min(1, "Say what the task worker must change.")
    .refine(...textLimit("note")),
});
export type SendBackForm = z.infer<typeof sendBackSchema>;

export const feedbackSchema = z.object({
  to: z.string().min(1, "Choose who reads the feedback."),
  body: z
    .string()
    .trim()
    .min(1, "Write the feedback.")
    .refine(...textLimit("feedback")),
});
export type FeedbackForm = z.infer<typeof feedbackSchema>;
