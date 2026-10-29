import * as RadixPopover from "@radix-ui/react-popover";
import * as RadixTabs from "@radix-ui/react-tabs";
import { useQuery } from "@tanstack/react-query";
import {
  BellOff,
  ChevronRight,
  CloudOff,
  FileWarning,
  FileX,
  FolderClock,
  GitBranch,
  GitMerge,
  GitPullRequest,
  Hand,
  Inbox as InboxIcon,
  MessagesSquare,
  SearchX,
  ShieldAlert,
  Unplug,
  UserRound,
  X,
  type LucideIcon,
} from "lucide-react";
import { useState } from "react";
import { needsYouQuery, sessionQuery } from "@/api/queries";
import type { NeedsYouKind } from "@/api/workers";
import { Count } from "@/components/badge/Chip";
import { buttonClasses } from "@/components/button/buttonStyles";
import { CodeText } from "@/components/code/CodeText";
import { ID_CLASSES } from "@/components/id/idStyles";
import {
  SEGMENT_LIST,
  segmentClasses,
} from "@/components/segments/segmentStyles";
import { Tooltip } from "@/components/overlay/Tooltip";
import { columnName } from "@/features/notifications/moves";
import { unreadCount } from "@/features/notifications/feed";
import { notificationStore, useFeed } from "@/features/notifications/store";
import {
  movesByTask,
  newText,
  type TaskMoves,
} from "@/features/notifications/updates";
import { formatAsOfMinute } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing, insetFocusRing } from "@/lib/focus";
import { useLiveStatus } from "@/live/status";
import { useNow } from "@/live/useNow";
import { idHref } from "@/routes/paths";
import { PersonName } from "@/people/PersonName";
import { HrefLink } from "./HrefLink";
import { listNote, listState } from "./listNote";
import { PanelNote } from "./PanelNote";
import {
  fullTitle,
  itemsFor,
  needsYouGroups,
  type NeedsYouLine,
} from "./needsYouModel";
import { inboxLabel, inboxTab, type InboxTab } from "./inboxModel";

const ICONS: Record<NeedsYouKind, LucideIcon> = {
  question: MessagesSquare,
  permission: ShieldAlert,
  human_review: GitPullRequest,
  request_draft: InboxIcon,
  human_task: UserRound,
  blocker: Hand,
  human_change: GitMerge,
  head_moved: GitBranch,
  folder_held: FolderClock,
  unstash_conflicted: FileWarning,
  checks_file_invalid_on_main: FileX,
  worker_unreachable: Unplug,
  worker_model_unreachable: CloudOff,
  claude_not_found: SearchX,
  notify_target_off: BellOff,
};

const MINUTE_MS = 60_000;

/** One row's frame: the icon, the words that may wrap, the time that stays on one line, and the chevron of a row that opens something. */
const ROW = "flex items-start gap-10 rounded-s px-10 py-8";
const ID = `${ID_CLASSES} font-semibold`;

function NeedLine({
  line,
  now,
  onOpen,
}: {
  line: NeedsYouLine;
  now: number;
  onOpen: () => void;
}) {
  const Icon = ICONS[line.kind];
  const full = fullTitle(line);
  const content = (
    <>
      <Icon
        aria-hidden="true"
        size={16}
        className="mt-2 shrink-0 text-text-2"
      />
      <span className="flex min-w-0 flex-1 flex-wrap items-baseline gap-x-10 gap-y-2">
        <span
          className={cn(
            "min-w-0 grow basis-2/3 text-14 wrap-anywhere text-text",
            line.inline && "line-clamp-2",
          )}
        >
          {line.person ? (
            <>
              <PersonName
                name={line.person}
                className="font-semibold whitespace-nowrap"
              />{" "}
            </>
          ) : null}
          {line.chip ? (
            <>
              <span className={ID}>{line.chip}</span>{" "}
            </>
          ) : null}
          <CodeText text={line.text} />
          {line.task && !line.inline ? (
            <>
              {" on "}
              <span className={ID}>{line.task}</span>
            </>
          ) : null}
          {line.inline && line.title ? (
            <>
              {": "}
              <span>
                <CodeText text={line.title} />
              </span>
            </>
          ) : null}
        </span>
        <span className="shrink-0 text-13 whitespace-nowrap text-text-2 tabular-nums">
          {formatAsOfMinute(now, line.since)}
        </span>
        {!line.inline && line.title ? (
          // A row that opens nothing has no tooltip to give a cut title, so its title wraps whole.
          <span
            className={cn(
              "min-w-0 basis-full text-13 text-text-2",
              line.href === null ? "wrap-anywhere" : "truncate",
            )}
          >
            <CodeText text={line.title} caption />
          </span>
        ) : null}
      </span>
    </>
  );
  if (line.href === null) {
    return (
      <li>
        <div className={ROW}>
          {content}
          <span aria-hidden="true" className="w-16 shrink-0" />
        </div>
      </li>
    );
  }
  const link = (
    <HrefLink
      href={line.href}
      onClick={onOpen}
      className={cn(ROW, "hover:bg-sunken", insetFocusRing)}
    >
      {content}
      <ChevronRight
        aria-hidden="true"
        size={16}
        className="mt-2 shrink-0 text-text-2"
      />
    </HrefLink>
  );
  return (
    <li>{full === null ? link : <Tooltip content={full}>{link}</Tooltip>}</li>
  );
}

function MovesRow({
  slug,
  row,
  fresh,
  now,
  onOpen,
}: {
  slug: string;
  row: TaskMoves;
  fresh: boolean;
  now: number;
  onOpen: () => void;
}) {
  const last = row.path.length - 1;
  return (
    <li className="flex items-start gap-10 rounded-s py-8 pr-4 pl-10 hover:bg-sunken">
      <span
        aria-hidden="true"
        className={cn(
          "mt-7 size-8 shrink-0 rounded-full",
          fresh ? "bg-accent" : "bg-transparent",
        )}
      />
      <HrefLink
        href={idHref(slug, "task", row.task)}
        onClick={onOpen}
        className={cn(
          "flex min-w-0 flex-1 flex-wrap items-baseline gap-x-10 gap-y-2 rounded-s",
          focusRing,
        )}
      >
        <span className="min-w-0 grow basis-2/3 text-14 wrap-anywhere text-text">
          <span className={ID}>{row.task}</span> moved{" "}
          <span className="sr-only">
            {row.path.map(columnName).join(" to ")}
          </span>
          <span aria-hidden="true">
            {row.path.map((status, index) => (
              <span key={index}>
                {index > 0 ? (
                  <span className="text-text-2">{" → "}</span>
                ) : null}
                <span
                  className={cn(
                    "whitespace-nowrap",
                    index === last && "font-semibold",
                  )}
                >
                  {columnName(status)}
                </span>
              </span>
            ))}
          </span>
        </span>
        <span className="shrink-0 text-13 whitespace-nowrap text-text-2 tabular-nums">
          {formatAsOfMinute(now, row.at)}
        </span>
        {row.title === null ? null : (
          <span className="min-w-0 basis-full text-13 wrap-anywhere text-text-2">
            {row.title}
          </span>
        )}
      </HrefLink>
      <button
        type="button"
        aria-label={`Clear the moves of ${row.task}`}
        onClick={() => {
          notificationStore.clearTask(slug, row.task);
        }}
        className={cn(
          "grid size-24 shrink-0 place-items-center rounded-s text-text-2 hover:bg-line hover:text-text",
          focusRing,
        )}
      >
        <X aria-hidden="true" size={14} />
      </button>
    </li>
  );
}

/** A tab of the Inbox as a segment of the shared switch, the two sharing the width. */
function tabClasses(current: boolean): string {
  return cn(
    segmentClasses(current),
    "inline-flex flex-1 items-center justify-center gap-6 whitespace-nowrap",
    focusRing,
  );
}

/** The top bar's one place for what waits for the human (Needs you) and the card moves since they last looked (Updates). Its gold count holds only what waits. */
export function Inbox({ slug }: { slug: string }) {
  const needs = useQuery(needsYouQuery(slug));
  const session = useQuery(sessionQuery());
  const feed = useFeed(slug);
  const status = useLiveStatus();
  const now = useNow(status.state === "reconnecting", MINUTE_MS);
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState<InboxTab>("needs");
  const [fresh, setFresh] = useState<ReadonlySet<string>>(new Set());
  const items = itemsFor(needs.data ?? [], session.data?.human ?? null);
  const groups = needsYouGroups(slug, items);
  const note = listNote(listState(needs), groups.length, {
    loading: "Loading what waits for you",
    failed: "Needs you could not load.",
    empty: "Nothing waits for you.",
  });
  const rows = movesByTask(feed);
  const unread = unreadCount(feed);
  const close = () => {
    setOpen(false);
  };
  const showUpdates = () => {
    setFresh(new Set(feed.filter((e) => e.unread).map((e) => e.task)));
    void notificationStore.refill(slug);
    notificationStore.markRead(slug);
  };
  const changeTab = (next: InboxTab) => {
    setTab(next);
    if (next === "updates") showUpdates();
  };
  return (
    <RadixPopover.Root
      open={open}
      onOpenChange={(next) => {
        if (next) {
          const first = inboxTab(items.length);
          setTab(first);
          if (first === "updates") showUpdates();
        } else if (tab === "updates") {
          notificationStore.markRead(slug);
        }
        setOpen(next);
      }}
    >
      <RadixPopover.Trigger asChild>
        <button
          type="button"
          aria-label={inboxLabel(items.length, unread)}
          className={cn(
            buttonClasses("quiet", "md"),
            "shrink-0 max-wide:min-h-44 max-wide:px-10",
            focusRing,
          )}
        >
          <InboxIcon aria-hidden="true" size={16} className="wide:hidden" />
          <span className="max-wide:sr-only">Inbox</span>
          {items.length > 0 ? <Count value={items.length} /> : null}
        </button>
      </RadixPopover.Trigger>
      <RadixPopover.Portal>
        <RadixPopover.Content
          align="end"
          sideOffset={6}
          collisionPadding={16}
          aria-label="Inbox"
          className="z-pop flex max-h-(--radix-popover-content-available-height) w-inbox max-w-(--width-popover-max) flex-col overflow-hidden rounded-m bg-surface shadow-pop outline-none"
        >
          <RadixTabs.Root
            value={tab}
            onValueChange={(value) => {
              changeTab(value === "updates" ? "updates" : "needs");
            }}
            className="flex min-h-0 flex-col"
          >
            <RadixTabs.List
              aria-label="Inbox"
              className={cn(
                SEGMENT_LIST,
                "mx-12 mt-12 mb-4 shrink-0 flex-nowrap",
              )}
            >
              <RadixTabs.Trigger
                value="needs"
                className={tabClasses(tab === "needs")}
              >
                Needs you
                {items.length > 0 ? <Count value={items.length} /> : null}
              </RadixTabs.Trigger>
              <RadixTabs.Trigger
                value="updates"
                className={tabClasses(tab === "updates")}
              >
                Updates
                {newText(unread) === null ? null : (
                  <span className="text-13 font-medium text-text-2">
                    {newText(unread)}
                  </span>
                )}
              </RadixTabs.Trigger>
            </RadixTabs.List>
            <RadixTabs.Content
              value="needs"
              className={cn(
                "min-h-0 overflow-y-auto px-6 pb-10",
                insetFocusRing,
              )}
            >
              {note !== null ? (
                <PanelNote
                  note={note}
                  onRetry={() => void needs.refetch()}
                  className="px-10 py-12"
                />
              ) : (
                groups.map((group) => (
                  <section key={group.id} aria-label={group.title}>
                    <h3 className="px-10 pt-12 pb-4 font-title text-13 font-semibold text-text-2">
                      {group.title}
                    </h3>
                    <ul>
                      {group.lines.map((line) => (
                        <NeedLine
                          key={line.key}
                          line={line}
                          now={now}
                          onOpen={close}
                        />
                      ))}
                    </ul>
                  </section>
                ))
              )}
            </RadixTabs.Content>
            <RadixTabs.Content
              value="updates"
              className={cn("flex min-h-0 flex-col", insetFocusRing)}
            >
              {rows.length === 0 ? (
                <p className="px-16 py-12 text-14 text-text-2">No card moves</p>
              ) : (
                <>
                  <ul className="min-h-0 overflow-y-auto px-6 pt-4 pb-6">
                    {rows.map((row) => (
                      <MovesRow
                        key={row.task}
                        slug={slug}
                        row={row}
                        fresh={fresh.has(row.task)}
                        now={now}
                        onOpen={close}
                      />
                    ))}
                  </ul>
                  <div className="flex shrink-0 justify-end px-12 pt-4 pb-10">
                    <button
                      type="button"
                      onClick={() => {
                        notificationStore.clearAll(slug);
                      }}
                      className={cn(
                        "rounded-s px-6 py-4 font-title text-14 font-semibold text-accent hover:bg-sunken",
                        focusRing,
                      )}
                    >
                      Clear all
                    </button>
                  </div>
                </>
              )}
            </RadixTabs.Content>
          </RadixTabs.Root>
        </RadixPopover.Content>
      </RadixPopover.Portal>
    </RadixPopover.Root>
  );
}
