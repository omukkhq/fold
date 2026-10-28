import { useState, type ReactNode } from "react";
import type { WorkspaceEntry } from "../../api/workspaces";
import { formatSince } from "../../format/time";
import { cn } from "../../lib/cn";
import { focusRing } from "../../lib/focus";
import type { LiveWorkspace } from "../../live/envelope";
import { ChangedFileRow } from "./ChangedFileRow";
import { visibleFiles } from "./files";
import { IdLink } from "./IdLink";
import type { RecordedItem } from "./recorded";
import { summarize } from "./summary";

export type ZoneTone = "working" | "recorded" | "main";

const SWATCH: Record<ZoneTone, string> = {
  working: "bg-primary",
  recorded: "bg-text-muted",
  main: "bg-ink",
};

const HEAD_TEXT: Record<ZoneTone, string> = {
  working: "text-primary-text",
  recorded: "text-text-muted",
  main: "text-text-muted",
};

function Swatch({ tone }: { tone: ZoneTone }) {
  return (
    <span
      aria-hidden="true"
      className={`size-10 shrink-0 rounded-xs ${SWATCH[tone]}`}
    />
  );
}

/** One key of the page legend; plain text, so the zone headings stay the only headings below the page title. */
export function LegendItem({
  tone,
  children,
}: {
  tone: ZoneTone;
  children: string;
}) {
  return (
    <li className="flex items-center gap-6 text-meta text-text-muted">
      <Swatch tone={tone} />
      {children}
    </li>
  );
}

export function ZoneHead({
  tone,
  children,
}: {
  tone: ZoneTone;
  children: ReactNode;
}) {
  return (
    <div className="flex items-center gap-8">
      <Swatch tone={tone} />
      <h3 className={`text-meta font-semibold ${HEAD_TEXT[tone]}`}>
        {children}
      </h3>
    </div>
  );
}

interface WorkingZoneProps {
  slug: string;
  entry: WorkspaceEntry;
  live: LiveWorkspace;
}

export function WorkingZone({ slug, entry, live }: WorkingZoneProps) {
  const [expanded, setExpanded] = useState(false);
  const { shown, hidden } = visibleFiles(live.files, expanded);
  return (
    <section
      aria-label={`Working changes in ${entry.id}`}
      className="flex min-w-0 flex-1 flex-col gap-4"
    >
      <ZoneHead tone="working">
        Working changes · not recorded · {summarize(live.files)}
      </ZoneHead>
      {live.files.length === 0 ? (
        <p className="text-bar text-text-muted">No edits yet.</p>
      ) : (
        <ul className="flex flex-col">
          {shown.map((file) => (
            <ChangedFileRow
              key={file.path}
              slug={slug}
              base={live.base}
              workspace={entry.id}
              file={file}
            />
          ))}
        </ul>
      )}
      {hidden > 0 ? (
        <button
          type="button"
          onClick={() => {
            setExpanded(true);
          }}
          className={cn(
            "self-start rounded-s text-meta text-text-muted hover:text-text",
            focusRing,
          )}
        >
          Show {hidden} more
        </button>
      ) : null}
    </section>
  );
}

interface RecordedZoneProps {
  entry: WorkspaceEntry;
  items: readonly RecordedItem[];
  now: number;
}

export function RecordedZone({ entry, items, now }: RecordedZoneProps) {
  return (
    <section
      aria-label={`Recorded changes in ${entry.id}`}
      className="flex w-full flex-col gap-6 md:w-1/3"
    >
      <ZoneHead tone="recorded">Recorded changes</ZoneHead>
      {items.length === 0 ? (
        <p className="text-bar text-text-muted">Nothing recorded yet.</p>
      ) : (
        <ul className="flex flex-col gap-6">
          {items.map((item) => (
            <li key={item.id} className="flex items-center gap-8">
              <IdLink href={item.href}>{item.id}</IdLink>
              <span
                className="min-w-0 flex-1 truncate text-bar text-text"
                title={item.text}
              >
                {item.text}
              </span>
              <span className="text-meta whitespace-nowrap text-text-muted">
                {formatSince(now, item.at)}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

export function MainZone({
  entry,
  text,
}: {
  entry: WorkspaceEntry;
  text: string;
}) {
  return (
    <section
      aria-label={`Main for ${entry.id}`}
      className="flex items-center gap-8 border-t border-border-subtle px-16 py-8"
    >
      <ZoneHead tone="main">Main</ZoneHead>
      <p className="text-bar text-text-muted">{text}</p>
    </section>
  );
}
