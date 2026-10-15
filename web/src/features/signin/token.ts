import { z } from "zod";

export const TOKEN_REQUIRED = "Enter a token.";

/** The token form's schema. The daemon decides whether a token is valid; the form only refuses an empty one. */
export const tokenFormSchema = z.object({
  token: z.string().trim().min(1, { error: TOKEN_REQUIRED }),
});

export type TokenFormValues = z.infer<typeof tokenFormSchema>;
