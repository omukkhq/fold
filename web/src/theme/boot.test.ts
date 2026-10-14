import { afterEach, describe, expect, it, vi } from "vitest";
import html from "../../index.html?raw";
import { THEME_STORAGE_KEY, parseStoredTheme, themeAttribute } from "./theme";

const script = /<script>([\s\S]*?)<\/script>/.exec(html)?.[1] ?? "";

interface Root {
  dataset: Record<string, string>;
}

/** Runs the inline script in global scope, as the browser does, with stand-ins for its two globals. */
function boot(getItem: (key: string) => string | null): string | undefined {
  const root: Root = { dataset: {} };
  vi.stubGlobal("localStorage", { getItem });
  vi.stubGlobal("document", { documentElement: root });
  globalThis.eval(script);
  return root.dataset.theme;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the theme script in index.html", () => {
  it("is present", () => {
    expect(script).toContain(THEME_STORAGE_KEY);
  });

  it.each([null, "light", "dark", "system", "sepia", ""])(
    "applies the theme module's rule to a stored %j",
    (raw) => {
      const stored = (key: string) => (key === THEME_STORAGE_KEY ? raw : null);
      expect(boot(stored) ?? null).toBe(themeAttribute(parseStoredTheme(raw)));
    },
  );

  it("leaves the system theme when storage throws", () => {
    expect(
      boot(() => {
        throw new Error("blocked");
      }),
    ).toBeUndefined();
  });
});
