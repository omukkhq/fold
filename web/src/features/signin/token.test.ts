import { describe, expect, it } from "vitest";
import { TOKEN_REQUIRED, tokenFormSchema } from "./token";

describe("tokenFormSchema", () => {
  it("trims a token", () => {
    expect(tokenFormSchema.parse({ token: "  fold_tk_abc " })).toEqual({
      token: "fold_tk_abc",
    });
  });

  it("refuses an empty token", () => {
    const result = tokenFormSchema.safeParse({ token: "   " });
    expect(result.success).toBe(false);
    expect(result.error?.issues[0]?.message).toBe(TOKEN_REQUIRED);
  });
});
