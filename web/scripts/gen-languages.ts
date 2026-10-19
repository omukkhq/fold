import { writeFile } from "node:fs/promises";
import {
  buildCatalog,
  renderCatalog,
  shikiLanguages,
} from "../src/syntax/catalogSource";

await writeFile(
  new URL("../src/syntax/catalog.gen.ts", import.meta.url),
  renderCatalog(buildCatalog(await shikiLanguages())),
);
