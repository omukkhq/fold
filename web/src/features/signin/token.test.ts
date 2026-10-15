import { describe, expect, it } from "vitest";
import { TOKEN_REQUIRED, tokenFormSchema } from "./token";
import { nameFormSchema } from "./name";

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

describe("nameFormSchema", () => {
  it("accepts a valid name", () => {
    expect(nameFormSchema.safeParse({ name: "tanim" }).success).toBe(true);
  });

  it("reports an invalid name on the name field", () => {
    for (const name of ["", "a".repeat(33), "Tanim", "-a"]) {
      const result = nameFormSchema.safeParse({ name });
      expect(result.success).toBe(false);
      expect(result.error?.issues[0]?.path).toEqual(["name"]);
    }
  });
});
