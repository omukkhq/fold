import { z } from "zod";

export const NAME_MAX_LENGTH = 32;

/** The line under the name prompt that states the name rule. */
export const NAME_HINT = `Up to ${String(NAME_MAX_LENGTH)} lowercase letters, digits, dashes and underscores.`;

export type NameProblem = "empty" | "too_long" | "characters" | "start";

export type NameCheck =
  { ok: true; name: string } | { ok: false; problem: NameProblem };

/** The trimmed name when the daemon accepts it as a human name: 1 to 32 characters of `a-z`, `0-9`, `-` and `_`, starting with a letter or digit. The form checks it first so it never sends a name the daemon refuses with `invalid_name`. */
export function validateName(raw: string): NameCheck {
  const name = raw.trim();
  if (name === "") {
    return { ok: false, problem: "empty" };
  }
  if (name.length > NAME_MAX_LENGTH) {
    return { ok: false, problem: "too_long" };
  }
  if (!/^[a-z0-9_-]+$/.test(name)) {
    return { ok: false, problem: "characters" };
  }
  if (!/^[a-z0-9]/.test(name)) {
    return { ok: false, problem: "start" };
  }
  return { ok: true, name };
}

const MESSAGES: Record<NameProblem, string> = {
  empty: "Enter a name.",
  too_long: `Use at most ${String(NAME_MAX_LENGTH)} characters.`,
  characters: "Use only lowercase letters, digits, dashes and underscores.",
  start: "Start with a letter or a digit.",
};

export function nameProblemMessage(problem: NameProblem): string {
  return MESSAGES[problem];
}

/** The form's schema: the same rules as `validateName`, reported on the `name` field. The submitted value is trimmed by the caller. */
export const nameFormSchema = z
  .object({ name: z.string() })
  .superRefine((values, ctx) => {
    const check = validateName(values.name);
    if (!check.ok) {
      ctx.addIssue({
        code: "custom",
        path: ["name"],
        message: nameProblemMessage(check.problem),
      });
    }
  });

export type NameFormValues = z.infer<typeof nameFormSchema>;
