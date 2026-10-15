import { z } from "zod";
import { LABEL_NAMES } from "@/components/label/labelColors";
import { parseTaskParam } from "@/routes/params";

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
  description: z
    .string()
    .max(20_000, "Keep the description to 20000 characters."),
});
export type NewTaskForm = z.infer<typeof newTaskSchema>;

export const descriptionSchema = z.object({
  description: z
    .string()
    .max(20_000, "Keep the description to 20000 characters."),
});
export type DescriptionForm = z.infer<typeof descriptionSchema>;

export const labelSchema = z.object({
  name: z
    .string()
    .trim()
    .min(1, "Write a label name.")
    .max(40, "Keep the name to 40 characters."),
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

export const feedbackSchema = z.object({
  to: z.string().min(1, "Choose who reads the feedback."),
  body: z
    .string()
    .trim()
    .min(1, "Write the feedback.")
    .max(10_000, "Keep the feedback to 10000 characters."),
});
export type FeedbackForm = z.infer<typeof feedbackSchema>;
