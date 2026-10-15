import type { FileNode, RepositoryFixture } from "../src/api/repository";

const now = Date.now();
const MINUTE = 60_000;

const r1: Record<string, FileNode> = {
  "README.md": { kind: "text", content: "# demo\n\nA demo project.\n" },
  "package.json": {
    kind: "text",
    content: '{\n  "name": "demo",\n  "private": true\n}\n',
  },
  "src/index.ts": { kind: "text", content: 'export const demo = "demo";\n' },
};
const r2 = {
  ...r1,
  "src/api/server.ts": {
    kind: "text",
    content: "export function serve(): void {}\n",
  },
} satisfies Record<string, FileNode>;
const r3 = {
  ...r2,
  "assets/logo.png": { kind: "binary", size: 18_432, hash: "b3c1d2e4f5a60718" },
} satisfies Record<string, FileNode>;
const r4 = {
  ...r3,
  "src/forms/use-form.ts": {
    kind: "text",
    content: "export function useForm(): void {}\n",
  },
  "src/forms/validators.ts": {
    kind: "text",
    content: "export const required = (v: string) => v !== '';\n",
  },
} satisfies Record<string, FileNode>;
const r5 = {
  ...r4,
  "docs/latest": { kind: "symlink", target: "../README.md" },
} satisfies Record<string, FileNode>;

export const repositories: Record<string, RepositoryFixture> = {
  demo: {
    revisions: [
      {
        id: "r1",
        number: 1,
        integration: {
          kind: "initial",
          title: "Imported from ~/src/demo",
          human: "human:tanim",
        },
        integratedAt: now - 300 * MINUTE,
      },
      {
        id: "r2",
        number: 2,
        integration: {
          kind: "proposal",
          proposal: "P-1",
          task: "T-9",
          title: "Scaffold the API service",
          agent: "backend",
        },
        integratedAt: now - 240 * MINUTE,
      },
      {
        id: "r3",
        number: 3,
        integration: {
          kind: "pull",
          remoteCommit: "9f2c1ab",
          human: "human:tanim",
        },
        integratedAt: now - 120 * MINUTE,
      },
      {
        id: "r4",
        number: 4,
        integration: {
          kind: "proposal",
          proposal: "P-5",
          task: "T-15",
          title: "Form field components",
          agent: "frontend",
        },
        integratedAt: now - 30 * MINUTE,
      },
      {
        id: "r5",
        number: 5,
        integration: {
          kind: "proposal",
          proposal: "P-7",
          task: "T-24",
          title: "Link the docs",
          agent: "review",
        },
        integratedAt: now - 5 * MINUTE,
      },
    ],
    files: { r1, r2, r3, r4, r5 },
  },
  ledger: {
    revisions: [
      {
        id: "r1",
        number: 1,
        integration: {
          kind: "initial",
          title: "Imported from ~/src/ledger",
          human: "human:tanim",
        },
        integratedAt: now - 60 * MINUTE,
      },
    ],
    files: { r1 },
  },
};
