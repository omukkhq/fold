import type { Integration, Revision } from "../../api/repository";
import { formatAsOf } from "../../format/time";

export interface RevisionLines {
  title: string;
  /** Mono meta line: ids, who, and the time of day. */
  meta: string;
}

/** What a revision row shows: a proposal integration names its proposal and task, a pull names the remote commit and the human. */
export function describeRevision(
  revision: Revision,
  now: number,
): RevisionLines {
  const at = formatAsOf(now, revision.integratedAt);
  return {
    title: titleOf(revision.integration),
    meta: metaOf(revision.integration, at),
  };
}

function titleOf(integration: Integration): string {
  switch (integration.kind) {
    case "proposal":
      return `${integration.proposal} · ${integration.title}`;
    case "pull":
      return `pull · ${integration.remoteCommit}`;
    case "initial":
      return integration.title;
  }
}

function metaOf(integration: Integration, at: string): string {
  switch (integration.kind) {
    case "proposal":
      return `${integration.task} · ${integration.agent} · ${at}`;
    case "pull":
      return `${integration.human} · ${at}`;
    case "initial":
      return `${integration.human} · ${at}`;
  }
}

/** `14.2 KB` style sizes for the tree and the binary notice. */
export function formatSize(bytes: number): string {
  if (bytes < 1024) {
    return `${String(bytes)} B`;
  }
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(1)} KB`;
  }
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
