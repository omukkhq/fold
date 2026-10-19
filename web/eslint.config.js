import js from "@eslint/js";
import { defineConfig } from "eslint/config";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

export default defineConfig(
  {
    ignores: [
      "dist",
      "coverage",
      "node_modules",
      "src/routeTree.gen.ts",
      "src/api/schema.d.ts",
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.strictTypeChecked,
  reactHooks.configs.flat.recommended,
  {
    languageOptions: {
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    rules: {
      "@typescript-eslint/no-explicit-any": "error",
      "@typescript-eslint/ban-ts-comment": "error",
      "no-restricted-syntax": [
        "error",
        {
          selector:
            "TSAsExpression[expression.type='TSAsExpression'][expression.typeAnnotation.type='TSUnknownKeyword']",
          message: "`as unknown as` is not allowed.",
        },
        {
          selector: "ImportExpression[source.value=/(^|\\/)crates(\\/|$)/]",
          message: "web/ never reads from crates/.",
        },
      ],
      "no-restricted-globals": [
        "error",
        {
          name: "fetch",
          message: "Call the API through src/api/client.ts.",
        },
      ],
      "no-restricted-properties": [
        "error",
        ...["window", "globalThis", "self"].map((object) => ({
          object,
          property: "fetch",
          message: "Call the API through src/api/client.ts.",
        })),
      ],
      "no-restricted-imports": [
        "error",
        {
          patterns: [
            {
              regex: "(^|/)crates(/|$)",
              message: "web/ never reads from crates/.",
            },
          ],
        },
      ],
    },
  },
  {
    // The highlighter fetches its engine, a file of the bundle; the test setup serves that file from node_modules.
    files: [
      "src/api/client.ts",
      "src/syntax/shiki.ts",
      "src/vitest.setup.ts",
      "**/*.test.ts",
    ],
    rules: {
      "no-restricted-globals": "off",
      "no-restricted-properties": "off",
    },
  },
);
