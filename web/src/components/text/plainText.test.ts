import { describe, expect, it } from "vitest";
import { PREVIEW_CHARS, plainPreview, plainText } from "./plainText";

describe("plainText", () => {
  it("takes off strong, emphasis, strike and code backticks and keeps the words", () => {
    expect(
      plainText("The copy lines of **T-14** are __done__; *see* _it_ ~~now~~"),
    ).toBe("The copy lines of T-14 are done; see it now");
    expect(plainText("Run `fld record` or ``a ` b``.")).toBe(
      "Run fld record or a ` b.",
    );
  });

  it("keeps the text of a link and an image and the address of an autolink", () => {
    expect(
      plainText(
        `See [the guide](/p/demo/board), ![logo](a.png), [ref][1] and <${["https:", "//example.com"].join("")}>`,
      ),
    ).toBe(
      `See the guide, logo, ref and ${["https:", "//example.com"].join("")}`,
    );
  });

  it("takes off heading, quote, list and task marks and code fences and puts every line on one line", () => {
    expect(
      plainText(
        "## Done\n> quoted\n- one\n* two\n3. three\n- [x] four\n```ts\nconst x = 1;\n```",
      ),
    ).toBe("Done quoted one two three four const x = 1;");
  });

  it("keeps snake_case names, products, mentions and ids as written", () => {
    expect(
      plainText(
        "accounts.bootstrap_superadmin is 2 * 3 * 4 for @web-lead and #T-9, P-6",
      ),
    ).toBe(
      "accounts.bootstrap_superadmin is 2 * 3 * 4 for @web-lead and #T-9, P-6",
    );
  });

  it("keeps marks inside a code span as code text", () => {
    expect(plainText("Call `**kwargs` and `_private_`")).toBe(
      "Call **kwargs and _private_",
    );
  });

  it("drops half a pair that a cut left behind", () => {
    expect(plainText("The copy lines of **T-14 are `don…")).toBe(
      "The copy lines of T-14 are don…",
    );
  });
});

describe("plainPreview", () => {
  it("keeps a short text whole", () => {
    expect(plainPreview("**Done** for T-5.")).toBe("Done for T-5.");
  });

  it("cuts after the marks are gone, at a word, with an ellipsis", () => {
    const words = Array.from({ length: 60 }, () => "**word**").join(" ");
    const preview = plainPreview(words);
    expect(preview).not.toContain("*");
    expect(preview.endsWith("word…")).toBe(true);
    expect(preview.length).toBeLessThanOrEqual(PREVIEW_CHARS + 1);
    expect(plainPreview("abcdefghij", 4)).toBe("abcd…");
  });

  it("reads only the start of a very long text and still ends the preview with an ellipsis", () => {
    const line = "**a ".repeat(20_000);
    const started = performance.now();
    const preview = plainPreview(line);
    expect(performance.now() - started).toBeLessThan(200);
    expect(preview.endsWith("…")).toBe(true);
    expect(preview.length).toBeLessThanOrEqual(PREVIEW_CHARS + 1);
    expect(preview).not.toContain("*");
    const many = `${"[x](/a) ".repeat(400)}\nlast words`;
    expect(plainPreview(many).endsWith("…")).toBe(true);
    expect(plainPreview("short\n\n")).toBe("short");
  });
});
