import { queryOptions } from "@tanstack/react-query";
import { keys } from "../../api/keys";
import {
  getFile,
  getRepository,
  getTree,
  listRevisions,
} from "../../api/repository";

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

export const treeQuery = (slug: string, revision: string, dir: string) =>
  queryOptions({
    queryKey: [...keys.repository(slug), "tree", revision, dir] as const,
    queryFn: () => getTree(slug, revision, dir),
  });

export const fileQuery = (slug: string, revision: string, path: string) =>
  queryOptions({
    queryKey: [...keys.repository(slug), "file", revision, path] as const,
    queryFn: () => getFile(slug, revision, path),
  });
