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
        projectService: { allowDefaultProject: ["eslint.config.js"] },
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
);
