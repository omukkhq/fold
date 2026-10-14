function diffHref(
  slug: string,
  from: string,
  to: string,
  path?: string,
): string {
  const search = new URLSearchParams({ from, to });
  if (path !== undefined) {
    search.set("path", path);
  }
  return `/p/${slug}/diff?${search.toString()}`;
}

/** A working file: the workspace's base against its working files. */
export function workingFileHref(
  slug: string,
  base: string,
  workspace: string,
  path: string,
): string {
  return diffHref(slug, base, workspace, path);
}

/** A checkpoint is compared with the base the workspace started from. */
export function checkpointHref(
  slug: string,
  base: string,
  checkpoint: string,
): string {
  return diffHref(slug, base, checkpoint);
}

/** A changeset is compared with the revision it was recorded against. */
export function changesetHref(
  slug: string,
  base: string,
  changeset: string,
): string {
  return diffHref(slug, base, changeset);
}

export function taskHref(slug: string, task: string): string {
  return `/p/${slug}/t/${task}`;
}
