import { useQuery } from "@tanstack/react-query";
import { TriangleAlert } from "lucide-react";
import { repositoryQuery } from "@/api/repositoryQueries";
import { Badge } from "../../components/badge/Badge";
import { Chip } from "../../components/badge/Chip";
import { formatSince } from "../../format/time";
import { PersonName } from "../../people/PersonName";
import { attentionView, behindTag } from "./attention";
import { IdLink } from "@/components/id/IdLink";
import { taskHref } from "./links";
import { lastActivity, panelHeading, workerOf, type Panel } from "./panels";
import { recordedItems } from "./recorded";
import { EditingPill, MissingBadge } from "./StatePill";
import { RecordedZone, WorkingZone } from "./Zones";

interface WorkspacePanelProps {
  slug: string;
  panel: Panel;
  now: number;
}

function Dot() {
  return <span aria-hidden="true">·</span>;
}

export function WorkspacePanel({ slug, panel, now }: WorkspacePanelProps) {
  const { entry, live } = panel;
  const activity = lastActivity(panel);
  const attention = attentionView(entry.attention, activity, now);
  const worker = workerOf(panel);
  const base = live?.base ?? entry.base;
  const heading = panelHeading(entry);
  const behind = behindTag(base, useQuery(repositoryQuery(slug)).data);
  return (
    <section
      aria-label={`Workspace ${entry.id}`}
      className="flex flex-col rounded-m bg-surface"
    >
      <header className="flex flex-col gap-4 px-14 pt-14 pb-4 sm:px-18">
        <div className="flex min-w-0 flex-wrap items-center gap-x-10 gap-y-4">
          {worker === null ? (
            <span className="text-15 text-text-2">no worker</span>
          ) : (
            <PersonName name={worker} className="text-15 whitespace-nowrap" />
          )}
          <span aria-hidden="true" className="text-text-2 max-sm:hidden">
            ·
          </span>
          <h2 className="order-last w-full text-16 font-semibold text-text sm:order-none sm:w-auto sm:min-w-0 sm:flex-1 sm:truncate">
            {heading.title}
          </h2>
          <span className="ml-auto flex flex-wrap items-center gap-6 sm:ml-0">
            {entry.state === "missing" ? (
              <MissingBadge />
            ) : attention.stalled === null ? (
              <EditingPill text="active" />
            ) : (
              <Badge
                tone="stalled"
                after={
                  <>
                    <span aria-hidden="true">·</span>
                    <span>{attention.stalled}</span>
                  </>
                }
              />
            )}
            {attention.staleClaim ? <Badge tone="stale" /> : null}
          </span>
        </div>
        <p className="flex flex-wrap items-center gap-x-6 gap-y-2 text-13 text-text-2 tabular-nums">
          {heading.task === null ? null : (
            <>
              <IdLink href={taskHref(slug, heading.task)}>
                {heading.task}
              </IdLink>
              <Dot />
            </>
          )}
          <span className="whitespace-nowrap">{entry.id}</span>
          <Dot />
          <span className="whitespace-nowrap">base {base}</span>
          {behind === null ? null : <Chip>{behind}</Chip>}
          <Dot />
          <span className="whitespace-nowrap">
            last edit {formatSince(now, activity)}
          </span>
        </p>
      </header>
      {entry.watch === "degraded" ? (
        <p
          role="note"
          className="mx-14 mt-8 flex items-center gap-8 rounded-s bg-warn-soft px-12 py-8 text-14 text-warn sm:mx-18"
        >
          <TriangleAlert aria-hidden="true" size={16} className="shrink-0" />
          File watching is degraded. Changes in this workspace refresh on a 10
          second scan.
        </p>
      ) : null}
      {live === null ? (
        <p className="px-14 pt-8 pb-16 text-14 text-text-2 sm:px-18">
          The workspace directory is missing, so no changes are listed.
        </p>
      ) : (
        <div className="grid grid-cols-1 gap-12 px-14 pt-10 pb-16 sm:px-18 md:grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)] md:gap-24">
          <WorkingZone slug={slug} entry={entry} live={live} />
          <RecordedZone
            entry={entry}
            items={recordedItems(slug, live.base, live.checkpoints)}
            now={now}
          />
        </div>
      )}
    </section>
  );
}
