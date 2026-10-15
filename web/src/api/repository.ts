import { ApiError } from "./errors";
import { fromFixture } from "./fixture";

/** The integration that made a main revision. The schema has no repository types yet, so these are hand-written. */
export type Integration =
  | {
      kind: "proposal";
      proposal: string;
      task: string;
      title: string;
      agent: string;
    }
  | { kind: "pull"; remoteCommit: string; human: string }
  | { kind: "initial"; title: string; human: string };

export interface Revision {
  /** The public id, for example `r8`. */
  id: string;
  number: number;
  integration: Integration;
  /** UTC milliseconds. */
  integratedAt: number;
}

export interface TreeEntry {
  name: string;
  path: string;
  kind: "directory" | "file" | "symlink";
  /** Bytes; files only. */
  size?: number;
  /** The link text of a symlink. It is never followed. */
  target?: string;
}

export type FileView =
  | { kind: "text"; path: string; size: number; hash: string; content: string }
  | { kind: "binary"; path: string; size: number; hash: string };

/** A stored node of a tree, keyed by its path. A directory exists because a path runs through it. */
export type FileNode =
  | { kind: "text"; content: string }
  | { kind: "binary"; size: number; hash: string }
  | { kind: "symlink"; target: string };

export interface RepositoryFixture {
  revisions: Revision[];
  /** The files of each revision, by revision id and path. */
  files: Record<string, Record<string, FileNode>>;
}

const fixture = import.meta.env.DEV
  ? () => import("../../fixtures/repository")
  : undefined;

const EMPTY: RepositoryFixture = { revisions: [], files: {} };

async function load(slug: string): Promise<RepositoryFixture> {
  const found = await fromFixture(
    fixture,
    (m): RepositoryFixture | undefined => m.repositories[slug],
    undefined,
  );
  return found ?? EMPTY;
}

const encoder = new TextEncoder();

/** The direct children of `dir` (`""` is the root): directories first, then by name. */
export function directoryEntries(
  nodes: Record<string, FileNode>,
  dir: string,
): TreeEntry[] {
  const prefix = dir === "" ? "" : `${dir}/`;
  const found = new Map<string, TreeEntry>();
  for (const [path, node] of Object.entries(nodes)) {
    if (!path.startsWith(prefix)) {
      continue;
    }
    const rest = path.slice(prefix.length);
    const [name = "", ...below] = rest.split("/");
    if (below.length > 0) {
      found.set(name, { name, path: `${prefix}${name}`, kind: "directory" });
    } else if (node.kind === "symlink") {
      found.set(name, { name, path, kind: "symlink", target: node.target });
    } else {
      found.set(name, {
        name,
        path,
        kind: "file",
        size:
          node.kind === "text"
            ? encoder.encode(node.content).length
            : node.size,
      });
    }
  }
  return [...found.values()].sort((a, b) => {
    const rank = (e: TreeEntry) => (e.kind === "directory" ? 0 : 1);
    return rank(a) - rank(b) || a.name.localeCompare(b.name);
  });
}

function filesOf(
  repo: RepositoryFixture,
  revision: string,
): Record<string, FileNode> {
  const files = repo.files[revision];
  if (!files) {
    throw new ApiError("revision_not_found", `revision ${revision} not found`);
  }
  return files;
}

/** The main revisions, newest first. */
export async function listRevisions(slug: string): Promise<Revision[]> {
  const repo = await load(slug);
  return [...repo.revisions].sort((a, b) => b.number - a.number);
}

/** Rejects with `revision_not_found` for a revision the project does not have. */
export async function getTree(
  slug: string,
  revision: string,
  dir: string,
): Promise<TreeEntry[]> {
  return directoryEntries(filesOf(await load(slug), revision), dir);
}

/** Rejects with `revision_not_found` or `not_found`. A binary file carries its size and hash and no content. */
export async function getFile(
  slug: string,
  revision: string,
  path: string,
): Promise<FileView> {
  const node = filesOf(await load(slug), revision)[path];
  if (!node || node.kind === "symlink") {
    throw new ApiError("not_found", `${path} is not a file at ${revision}`);
  }
  if (node.kind === "binary") {
    return { kind: "binary", path, size: node.size, hash: node.hash };
  }
  return {
    kind: "text",
    path,
    size: encoder.encode(node.content).length,
    hash: `fixture-${String(node.content.length)}`,
    content: node.content,
  };
}
