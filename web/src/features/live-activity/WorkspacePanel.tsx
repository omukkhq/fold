import { useQuery } from "@tanstack/react-query";
import { repositoryQuery } from "@/api/repositoryQueries";
import { Badge } from "../../components/badge/Badge";
import { formatSince } from "../../format/time";
import { PersonName } from "../../people/PersonName";
import { attentionView } from "./attention";
import { IdLink } from "./IdLink";
import { taskHref } from "./links";
import { mainLine } from "./attention";
import { lastActivity, panelHeading, workerOf, type Panel } from "./panels";
import { recordedItems } from "./recorded";
import { EditingPill, MissingBadge } from "./StatePill";
import { MainZone, RecordedZone, WorkingZone } from "./Zones";

interface WorkspacePanelProps {
  slug: string;
  panel: Panel;
  now: number;
}

export function WorkspacePanel({ slug, panel, now }: WorkspacePanelProps) {
  const { entry, live } = panel;
  const activity = lastActivity(panel);
  const attention = attentionView(entry.attention, activity, now);
  const worker = workerOf(panel);
  const base = live?.base ?? entry.base;
  const heading = panelHeading(entry);
  const repository = useQuery(repositoryQuery(slug)).data;
  return (
    <section
      aria-label={`Workspace ${entry.id}`}
      className="flex flex-col rounded-l bg-surface inset-ring inset-ring-line"
    >
      <header className="flex flex-wrap items-center gap-12 border-b border-line px-16 py-12">
        <span className="flex items-center gap-6 font-mono text-13 text-text">
          {worker === null ? (
            "no worker"
          ) : (
            <PersonName name={worker} className="text-14" />
          )}
        </span>
        {heading.task === null ? null : (
          <IdLink href={taskHref(slug, heading.task)}>{heading.task}</IdLink>
        )}
        <h2 className="min-w-0 flex-1 truncate text-15 font-semibold text-text">
          {heading.title}
        </h2>
        <span className="font-mono text-13 text-text-2">
          {entry.id} · base {base}
        </span>
        <span className="text-13 text-text-2">
          {formatSince(now, activity)}
        </span>
        {entry.state === "missing" ? (
          <MissingBadge />
        ) : (
          <EditingPill text="active" />
        )}
        {attention.stalled === null ? null : (
          <>
            <Badge tone="stalled" />
            <span className="text-13 text-text-2">{attention.stalled}</span>
          </>
        )}
        {attention.staleClaim ? <Badge tone="stale" /> : null}
      </header>
      {entry.watch === "degraded" ? (
        <p
          role="note"
          className="border-b border-line bg-warn-soft px-16 py-8 text-14 text-warn"
        >
          File watching is degraded. Changes in this workspace refresh on a 10
          second scan.
        </p>
      ) : null}
      {live === null ? (
        <p className="px-16 py-12 text-14 text-text-2">
          The workspace directory is missing, so no changes are listed.
        </p>
      ) : (
        <div className="flex flex-col gap-24 px-16 pt-10 pb-14 md:flex-row">
          <WorkingZone slug={slug} entry={entry} live={live} />
          <RecordedZone
            entry={entry}
            items={recordedItems(slug, live.base, live.checkpoints)}
            now={now}
          />
        </div>
      )}
      <MainZone entry={entry} text={mainLine(base, repository)} />
    </section>
  );
}
