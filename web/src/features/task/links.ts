function base(slug: string): string {
  return `/p/${encodeURIComponent(slug)}`;
}

/** The diff viewer; without `from` it compares against the base recorded for `to`. */
export function diffPath(
  slug: string,
  diff: { to: string; from?: string; path?: string },
): string {
  const params = new URLSearchParams();
  if (diff.from !== undefined) {
    params.set("from", diff.from);
  }
  params.set("to", diff.to);
  if (diff.path !== undefined) {
    params.set("path", diff.path);
  }
  return `${base(slug)}/diff?${params.toString()}`;
}

export function proposalPath(slug: string, proposal: string): string {
  return `${base(slug)}/proposals/${encodeURIComponent(proposal)}`;
}

export function threadPath(slug: string, thread: string): string {
  return `${base(slug)}/messages?${new URLSearchParams({ thread }).toString()}`;
}

export function revisionPath(slug: string, revision: string): string {
  return `${base(slug)}/repository?${new URLSearchParams({ revision }).toString()}`;
}
