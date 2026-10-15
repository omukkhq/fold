import { describe, expect, it } from "vitest";
import {
  importsFixtures,
  importsRetiredFixture,
  mayImportFixtures,
} from "./fixtureGuard";

describe("importsFixtures", () => {
  it.each([
    'import { projects } from "../../fixtures/projects";',
    "import {x} from '../fixtures/session'",
    'const m = await import("../../fixtures/projects");',
    'import "../../fixtures/projects";',
    'const all = import.meta.glob("../../fixtures/*.ts");',
    "import.meta.glob<string>(['../fixtures/*.ts'], { eager: true })",
  ])("flags %s", (source) => {
    expect(importsFixtures(source)).toBe(true);
  });

  it.each([
    'import { keys } from "./keys";',
    'const fixtures = ["a"];',
    "// reads fixtures/projects in a comment",
  ])("passes %s", (source) => {
    expect(importsFixtures(source)).toBe(false);
  });
});

describe("mayImportFixtures", () => {
  it("allows the api seam and tests only", () => {
    expect(mayImportFixtures("/src/api/projects.ts")).toBe(true);
    expect(mayImportFixtures("/src/app/Shell.test.ts")).toBe(true);
    expect(mayImportFixtures("/src/app/Shell.tsx")).toBe(false);
    expect(mayImportFixtures("/src/routes/p.$slug.tsx")).toBe(false);
  });
});

describe("importsRetiredFixture", () => {
  it.each([
    'import { board } from "../../fixtures/board";',
    "import {x} from '../fixtures/tasks'",
    'const m = await import("../../fixtures/demoBoard");',
  ])("flags %s", (source) => {
    expect(importsRetiredFixture(source)).toBe(true);
  });

  it.each([
    'import { agents } from "../../fixtures/agents";',
    'import { projects } from "../../fixtures/projects";',
    'import { tasks } from "./tasks";',
  ])("passes %s", (source) => {
    expect(importsRetiredFixture(source)).toBe(false);
  });
});
