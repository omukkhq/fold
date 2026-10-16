import { queryOptions } from "@tanstack/react-query";
import { keys } from "./keys";
import { getWorkspaceTree } from "./workspaces";
import {
  getFile,
  getRepository,
  getRevision,
  getTree,
  listRevisions,
} from "./repository";

export const repositoryQuery = (slug: string) =>
  queryOptions({
    queryKey: [...keys.repository(slug), "state"] as const,
    queryFn: () => getRepository(slug),
  });

export const revisionsQuery = (slug: string) =>
  queryOptions({
    queryKey: [...keys.repository(slug), "revisions"] as const,
    queryFn: () => listRevisions(slug),
  });

export const revisionQuery = (slug: string, revision: string) =>
  queryOptions({
    queryKey: [...keys.repository(slug), "revision", revision] as const,
    queryFn: () => getRevision(slug, revision),
  });

export const treeQuery = (slug: string, revision: string, dir: string) =>
  queryOptions({
    queryKey: keys.revisionTree(slug, revision, dir),
    queryFn: () => getTree(slug, revision, dir),
    staleTime: Infinity,
  });

export const fileQuery = (slug: string, revision: string, path: string) =>
  queryOptions({
    queryKey: keys.revisionFile(slug, revision, path),
    queryFn: () => getFile(slug, revision, path),
    staleTime: Infinity,
  });

export const workspaceTreeQuery = (
  slug: string,
  workspace: string,
  dir: string,
) =>
  queryOptions({
    queryKey: [...keys.workspace(slug, workspace), "tree", dir] as const,
    queryFn: () => getWorkspaceTree(slug, workspace, dir),
  });
