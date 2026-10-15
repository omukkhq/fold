import { api } from "./client";
import { unwrap } from "./result";
import type { components } from "./schema";

type Schemas = components["schemas"];

export type WorkspaceState = Schemas["WorkspaceStateView"];
export type AttentionKind = Schemas["AttentionKind"];
/** The daemon decides attention; the web only shows it. */
export type AttentionItem = Schemas["AttentionItem"];
export interface ChangesetSummary {
  /** @example C-40 */
  id: string;
  message: string;
  /** UTC milliseconds. */
  recorded_at: number;
  /** The revision the changeset was recorded against, the `from` of its comparison. */
  base: string;
}

/** Fields the workspace routes leave out of `WorkspaceEntryView`. Each is optional, and the screen draws nothing for an absent one. */
export interface WorkspaceDetails {
  /** The changesets recorded in the workspace. */
  changesets?: ChangesetSummary[];
}

/** A workspace with its live state when the list was read (null when not attached) and the attention items of its task. */
export type WorkspaceEntry = Schemas["WorkspaceEntryView"] & WorkspaceDetails;
/** The working changes of an active workspace after a full scan, with its watch mode and the files the scan could not read. */
export type WorkspaceChanges = Schemas["WorkspaceChangesView"];

/** The project's active and missing task workspaces, by id. */
export async function listWorkspaces(slug: string): Promise<WorkspaceEntry[]> {
  const { workspaces } = await unwrap(
    api.GET("/api/v1/projects/{slug}/workspaces", {
      params: { path: { slug } },
    }),
  );
  return workspaces;
}

/** The entries of one directory (`""` is the root) of an active workspace's working files, in name order. Rejects with `workspace_not_active` for a workspace that is not active. */
export async function getWorkspaceTree(
  slug: string,
  id: string,
  dir: string,
): Promise<Schemas["WorkspaceTreeEntryView"][]> {
  const { entries } = await unwrap(
    api.GET("/api/v1/projects/{slug}/workspaces/{id}/tree", {
      params: { path: { slug, id }, query: { path: dir } },
    }),
  );
  return entries;
}

/** One workspace in any state. Rejects with `workspace_not_found`. */
export async function getWorkspace(
  slug: string,
  id: string,
): Promise<WorkspaceEntry> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/workspaces/{id}", {
      params: { path: { slug, id } },
    }),
  );
}

/** Rejects with `workspace_not_active` for a missing or closed workspace. */
export async function getWorkspaceChanges(
  slug: string,
  id: string,
): Promise<WorkspaceChanges> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/workspaces/{id}/changes", {
      params: { path: { slug, id } },
    }),
  );
}
