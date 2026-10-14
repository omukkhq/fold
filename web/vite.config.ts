import { fileURLToPath } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import { tanstackRouter } from "@tanstack/router-plugin/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";

const fixturesDir = fileURLToPath(new URL("./fixtures/", import.meta.url));

/** Fixture data is for development only: a production bundle that holds a fixture module fails the build. */
function noFixturesInBundle(): Plugin {
  return {
    name: "fold:no-fixtures-in-bundle",
    apply: "build",
    generateBundle(_options, bundle) {
      for (const output of Object.values(bundle)) {
        if (output.type !== "chunk") {
          continue;
        }
        const leaked = output.moduleIds.filter((id) =>
          id.startsWith(fixturesDir),
        );
        if (leaked.length > 0) {
          this.error(
            `${output.fileName} bundles fixture modules: ${leaked.join(", ")}`,
          );
        }
      }
    },
  };
}

export default defineConfig({
  plugins: [
    tanstackRouter({
      target: "react",
      routesDirectory: "./src/routes",
      generatedRouteTree: "./src/routeTree.gen.ts",
      autoCodeSplitting: true,
      // Route files are .tsx; a .ts file beside them is a tested helper, such as params or a search schema.
      routeFileIgnorePattern: "\\.ts$",
    }),
    react(),
    tailwindcss(),
    noFixturesInBundle(),
  ],
  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },
  server: {
    port: 5173,
    strictPort: true,
    proxy: { "/api": "http://127.0.0.1:7420" },
  },
});
