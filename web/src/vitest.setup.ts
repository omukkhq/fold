import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { configure } from "@testing-library/react";

// Checks run beside parallel builds on one machine; every wait ends on a condition, so a longer cap hides no sleep.
configure({ asyncUtilTimeout: 5000 });

// The page fetches the highlighter's WebAssembly from the bundle. Under test its URL is the file's path in node_modules,
// so that path is answered from disk, as a static server would; a test that stubs `fetch` sees the page offline.
const pageFetch = globalThis.fetch.bind(globalThis);
globalThis.fetch = async (input, init) => {
  const url = input instanceof Request ? input.url : String(input);
  if (url.startsWith("/node_modules/") && url.endsWith(".wasm")) {
    const bytes = await readFile(join(process.cwd(), url));
    return new Response(bytes, {
      headers: { "content-type": "application/wasm" },
    });
  }
  return pageFetch(input, init);
};
