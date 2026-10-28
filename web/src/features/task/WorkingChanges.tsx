import { FileDiff, GitCompare } from "lucide-react";
import type { LiveWorkspace, TaskWorkspaceInfo } from "@/api/tasks";
import { HrefLink } from "@/app/HrefLink";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { diffPath } from "./links";
import {
  changeLetter,
  changesNote,
  changeTone,
  lineCounts,
  liveChipText,
  type ChangeTone,
} from "./liveStrip";
import { LinkButton, SectionHead } from "./parts";

const TONES: Record<ChangeTone, string> = {
  added: "bg-accent-soft text-accent",
  deleted: "bg-bad-soft text-bad",
  other: "bg-surface text-text-2",
};

function FileRow({
  slug,
  workspace,
  file,
}: {
  slug: string;
  workspace: string;
  file: LiveWorkspace["files"][number];
}) {
  const counts = lineCounts(file);
  return (
    <li>
      <HrefLink
        href={diffPath(slug, { to: workspace, path: file.path })}
        className={cn(
          "flex items-center gap-10 rounded-s px-10 py-6 hover:bg-surface",
          focusRing,
        )}
      >
        <span className="sr-only">{file.change}</span>
        <span
          aria-hidden="true"
          className={cn(
            "inline-flex size-18 shrink-0 items-center justify-center rounded-s font-mono text-13 font-bold",
            TONES[changeTone(file.change)],
          )}
        >
          {changeLetter(file.change)}
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-13 text-text">
          {file.previous_path
            ? `${file.previous_path} → ${file.path}`
            : file.path}
        </span>
        {counts.added ? (
          <span className="font-mono text-13 text-accent">{counts.added}</span>
        ) : null}
        {counts.removed ? (
          <span className="font-mono text-13 text-bad">{counts.removed}</span>
        ) : null}
      </HrefLink>
    </li>
  );
}

/** The workspace's uncommitted changes. They are never history, so the section says they are not in main. */
export function WorkingChanges({
  slug,
  workspace,
  info,
  live,
  now,
}: {
  slug: string;
  /** The workspace id, `ws-9`. */
  workspace: string | null;
  /** The workspace details with the main head; null without a live workspace, which hides "Compare with main". */
  info: TaskWorkspaceInfo | null;
  live: LiveWorkspace | null;
  now: number;
}) {
  return (
    <section aria-label="Working changes" className="flex flex-col gap-10">
      <SectionHead title="Working changes">
        {workspace && live ? (
          <>
            <span className="inline-flex items-center gap-6 rounded-s bg-accent-soft px-8 py-2 text-13 font-medium text-accent">
              <span
                aria-hidden="true"
                className="size-7 rounded-full bg-accent"
              />
              {liveChipText(live, now)}
            </span>
            <span className="text-14 text-text-2">
              {changesNote(live, info)}
            </span>
          </>
        ) : null}
      </SectionHead>
      {workspace === null ? (
        <p className="text-14 text-text-2">
          No workspace yet. A worker opens one when it starts the task.
        </p>
      ) : live === null || live.files.length === 0 ? (
        <p className="text-14 text-text-2">
          No working changes in {workspace}.
        </p>
      ) : (
        <ul className="flex flex-col rounded-m bg-sunken p-6">
          {live.files.map((file) => (
            <FileRow
              key={file.path}
              slug={slug}
              workspace={workspace}
              file={file}
            />
          ))}
        </ul>
      )}
      {workspace ? (
        <div className="flex gap-8">
          <LinkButton href={diffPath(slug, { to: workspace })} icon={FileDiff}>
            Open diff
          </LinkButton>
          {info ? (
            <LinkButton
              href={diffPath(slug, { from: info.main, to: workspace })}
              variant="ghost"
              icon={GitCompare}
            >
              Compare with main
            </LinkButton>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}
