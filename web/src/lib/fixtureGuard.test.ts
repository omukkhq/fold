import { describe, expect, it } from "vitest";
import { importsFixtures } from "./fixtureGuard";

describe("importsFixtures", () => {
  it.each([
    'import { projects } from "../../fixtures/projects";',
    "import {x} from '../fixtures/session'",
    'const m = await import("../../fixtures/projects");',
    'import "../../fixtures/projects";',
    'import { board } from "../../fixtures/board";',
    'const e = await import("../../fixtures/events");',
    'const all = import.meta.glob("../../fixtures/*.ts");',
    "import.meta.glob<string>(['../fixtures/*.ts'], { eager: true })",
    'const g = import.meta.glob(["./a/*.ts", "../fixtures/*.ts"]);',
    'import { projects } from "@/../fixtures";',
    'import "../../fixtures";',
    'export * from "@/../fixtures/projects";',
    'const m = require("../../fixtures/projects");',
  ])("flags %s", (source) => {
    expect(importsFixtures(source)).toBe(true);
  });

  it.each([
    'import { keys } from "./keys";',
    'const fixtures = ["a"];',
    "// reads fixtures/projects in a comment",
    'import { table } from "./fixturesTable";',
    'const g = import.meta.glob(["./a/*.ts", "./b/*.ts"]);',
  ])("passes %s", (source) => {
    expect(importsFixtures(source)).toBe(false);
  });
});
