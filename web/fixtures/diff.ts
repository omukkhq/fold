import type {
  ComparedFile,
  ComparedState,
  ComparisonFixture,
  FileDiff,
  Hunk,
  HunkLine,
} from "../src/api/diff";
import { comparisons as proposalComparisons } from "./proposals";

const READ_AT = Date.UTC(2026, 9, 15, 12, 12, 31);

function hash(seed: string): string {
  return seed.repeat(64).slice(0, 64);
}

const ctx = (text: string): HunkLine => ({ kind: "context", text });
const add = (text: string): HunkLine => ({ kind: "add", text });
const del = (text: string): HunkLine => ({ kind: "remove", text });

function hunk(
  oldStart: number,
  newStart: number,
  section: string,
  lines: HunkLine[],
): Hunk {
  return {
    old_start: oldStart,
    old_lines: lines.filter((l) => l.kind !== "add").length,
    new_start: newStart,
    new_lines: lines.filter((l) => l.kind !== "remove").length,
    section,
    lines,
  };
}

interface Entry {
  file: ComparedFile;
  diff: FileDiff;
}

function text(
  path: string,
  change: ComparedFile["change"],
  hunks: Hunk[],
  hashes: { old: string | null; new: string | null },
  previousPath: string | null = null,
): Entry {
  const lines = hunks.flatMap((h) => h.lines);
  return {
    file: {
      path,
      previous_path: previousPath,
      change,
      added_lines: lines.filter((l) => l.kind === "add").length,
      removed_lines: lines.filter((l) => l.kind === "remove").length,
      binary: false,
      old_hash: hashes.old,
      new_hash: hashes.new,
    },
    diff: { path, binary: false, hunks },
  };
}

function binary(
  path: string,
  change: ComparedFile["change"],
  hashes: { old: string | null; new: string | null },
): Entry {
  return {
    file: {
      path,
      previous_path: null,
      change,
      added_lines: 0,
      removed_lines: 0,
      binary: true,
      old_hash: hashes.old,
      new_hash: hashes.new,
    },
    diff: { path, binary: true, hunks: [] },
  };
}

const login = text(
  "src/routes/login.tsx",
  "modified",
  [
    hunk(12, 12, "export function LoginRoute() {", [
      ctx("export function LoginRoute() {"),
      del("  const [error, setError] = useState<string>()"),
      del("  const onSubmit = async (e: FormEvent) => {"),
      add("  const form = useForm({ schema: loginSchema })"),
      add("  const login = useMutation({ mutationFn: signIn })"),
      add(
        "  const onSubmit = form.handleSubmit((values) => login.mutate(values))",
      ),
      ctx(""),
      ctx("  return ("),
      del("    <form onSubmit={onSubmit}>"),
      add("    <form onSubmit={onSubmit} aria-busy={login.isPending}>"),
      add("      <TextField {...form.field('email')} />"),
      add("      <PasswordField {...form.field('password')} />"),
      add("      {login.error && <FormError error={login.error} />}"),
      add(
        '      <Button type="submit" disabled={login.isPending}>Sign in</Button>',
      ),
      ctx("    </form>"),
    ]),
    hunk(40, 45, "function rememberedEmail() {", [
      ctx("function rememberedEmail() {"),
      ctx("  const stored = localStorage.getItem('email')"),
      ctx("  if (stored === null) {"),
      ctx("    return ''"),
      ctx("  }"),
      ctx("  try {"),
      ctx("    return JSON.parse(stored) as string"),
      ctx("  } catch {"),
      ctx("    return ''"),
      ctx("  }"),
      ctx("}"),
      ctx(""),
      ctx("function clearRemembered() {"),
      del("  localStorage.clear()"),
      add("  localStorage.removeItem('email')"),
      ctx("}"),
    ]),
  ],
  { old: hash("a1"), new: hash("b2") },
);

const schema = text(
  "src/forms/login-schema.ts",
  "added",
  [
    hunk(0, 1, "", [
      add("import { z } from 'zod'"),
      add(""),
      add("export const loginSchema = z.object({"),
      add("  email: z.string().email(),"),
      add("  password: z.string().min(8),"),
      add("})"),
      add(""),
      add("export type LoginValues = z.infer<typeof loginSchema>"),
    ]),
  ],
  { old: null, new: hash("c3") },
);

const auth = text(
  "src/api/auth.ts",
  "modified",
  [
    hunk(4, 4, "export async function signIn(values: LoginValues) {", [
      ctx("export async function signIn(values: LoginValues) {"),
      del(
        "  const res = await fetch('/api/login', { method: 'POST', body: JSON.stringify(values) })",
      ),
      add("  const res = await api.POST('/session', { body: values })"),
      ctx("  if (!res.ok) {"),
      del("    throw new Error('login failed')"),
      add("    throw toApiError(res.error)"),
      ctx("  }"),
    ]),
  ],
  { old: hash("d4"), new: hash("e5") },
);

const oldLogin = text(
  "src/routes/old-login.tsx",
  "deleted",
  [
    hunk(1, 0, "", [
      del("import { useState } from 'react'"),
      del(""),
      del("export function OldLogin() {"),
      del("  const [email, setEmail] = useState('')"),
      del(
        "  return <input value={email} onChange={(e) => setEmail(e.target.value)} />",
      ),
      del("}"),
    ]),
  ],
  { old: hash("f6"), new: null },
);

const session = text(
  "src/lib/session.ts",
  "renamed",
  [
    hunk(1, 1, "", [
      del("export const SESSION_KEY = 'auth-session'"),
      add("export const SESSION_KEY = 'session'"),
      ctx(""),
      ctx("export function readSession() {"),
    ]),
  ],
  { old: hash("07"), new: hash("18") },
  "src/lib/auth-session.ts",
);

const logo = binary("public/logo.png", "modified", {
  old: hash("29"),
  new: hash("3a"),
});

const movedDoc = hash("4b");
const docRemoved = text("docs/sign-in.md", "deleted", [], {
  old: movedDoc,
  new: null,
});
const docAdded = text("docs/login.md", "added", [], {
  old: null,
  new: movedDoc,
});

const icons = text(
  "src/generated/icons.ts",
  "added",
  [
    hunk(
      0,
      1,
      "",
      Array.from({ length: 1500 }, (_, i) =>
        add(`export const icon${String(i + 1)} = "icon-${String(i + 1)}"`),
      ),
    ),
  ],
  { old: null, new: hash("5c") },
);

const changelog = text(
  "CHANGELOG.md",
  "modified",
  [
    hunk(1, 1, "", [
      ctx("# Changelog"),
      ctx(""),
      add("## r8"),
      add(""),
      add("- Sign in with a form schema."),
      add(""),
      ctx("## r7"),
    ]),
  ],
  { old: hash("6d"), new: hash("7e") },
);

function revision(id: string): ComparedState {
  return { kind: "revision", id, base: null, task: null, worker: null };
}

function fixture(
  from: ComparedState,
  to: ComparedState,
  readAt: number | null,
  entries: Entry[],
): ComparisonFixture {
  return {
    comparison: {
      from,
      to,
      read_at: readAt,
      files: entries.map((e) => e.file),
    },
    diffs: entries.map((e) => e.diff),
  };
}

const workingChanges = fixture(
  revision("r7"),
  {
    kind: "workspace",
    id: "ws-7",
    base: "r7",
    task: "T-12",
    worker: "frontend/ui",
  },
  READ_AT,
  [login, schema, auth, oldLogin, session, logo, docRemoved, docAdded, icons],
);

/**
 * Keyed by `from..to`, or by `to` alone for `to` against its recorded base: the working changes of ws-7 in both forms, two main revisions,
 * a revision with itself, and every proposal version.
 */
export const comparisons: Record<string, ComparisonFixture> = {
  ...proposalComparisons,
  "r7..ws-7": workingChanges,
  "ws-7": workingChanges,
  "r7..r8": fixture(revision("r7"), revision("r8"), null, [changelog, login]),
  "r8..r8": fixture(revision("r8"), revision("r8"), null, []),
};
