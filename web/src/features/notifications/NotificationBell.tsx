import * as RadixPopover from "@radix-ui/react-popover";
import * as Toast from "@radix-ui/react-toast";
import { Bell, SquareKanban, X } from "lucide-react";
import { useState } from "react";
import { createPortal } from "react-dom";
import { COLUMNS } from "@/features/board/columns";
import { HrefLink } from "@/app/HrefLink";
import { formatAsOfMinute } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing, insetFocusRing } from "@/lib/focus";
import { useLiveStatus } from "@/live/status";
import { useNow } from "@/live/useNow";
import { idHref } from "@/routes/paths";
import { entryKey, unreadCount, type FeedEntry } from "./feed";
import { columnName } from "./moves";
import { notificationStore, useFeed, useMoveToasts } from "./store";
import { bellLabel, moveText } from "./text";

const MINUTE_MS = 60_000;

/** A toast closes by itself after this time; hovering or focusing it holds it open. */
const TOAST_MS = 5_000;

const DOTS = new Map(COLUMNS.map((column) => [column.status, column.dot]));

function Entry({
  slug,
  entry,
  fresh,
  now,
  onOpen,
}: {
  slug: string;
  entry: FeedEntry;
  fresh: boolean;
  now: number;
  onOpen: () => void;
}) {
  const text = moveText(entry);
  return (
    <li className={cn("flex items-center", fresh && "bg-primary-soft")}>
      <HrefLink
        href={idHref(slug, "task", entry.task)}
        onClick={onOpen}
        className={cn(
          "flex min-w-0 flex-1 items-center gap-10 py-8 pr-4 pl-16 hover:bg-sunken",
          insetFocusRing,
        )}
      >
        <span
          aria-hidden="true"
          className={cn("size-8 shrink-0 rounded-chip", DOTS.get(entry.to))}
        />
        <span className="flex min-w-0 flex-1 flex-col gap-2">
          <span className="text-body text-text">{text}</span>
          <span className="flex min-w-0 items-center gap-6 text-meta text-text-muted">
            {entry.title === null ? null : (
              <span className="min-w-0 truncate">{entry.title}</span>
            )}
            <span className="shrink-0">
              {columnName(entry.from)} → {columnName(entry.to)} ·{" "}
              {formatAsOfMinute(now, entry.at)}
            </span>
          </span>
        </span>
      </HrefLink>
      <button
        type="button"
        aria-label={`Clear ${text}`}
        onClick={() => {
          notificationStore.clear(slug, entryKey(entry));
        }}
        className={cn(
          "mr-8 rounded-s p-6 text-text-faint hover:bg-sunken hover:text-text",
          focusRing,
        )}
      >
        <X aria-hidden="true" size={14} />
      </button>
    </li>
  );
}

function MoveToasts({ slug }: { slug: string }) {
  const toasts = useMoveToasts().filter((toast) => toast.slug === slug);
  return (
    <Toast.Provider label="Card moves" duration={TOAST_MS}>
      {toasts.map((toast) => (
        <Toast.Root
          key={toast.id}
          type="background"
          onOpenChange={(open) => {
            if (!open) {
              notificationStore.dismiss(toast.id);
            }
          }}
          className="flex items-start gap-10 rounded-m bg-ink px-14 py-10 text-on-ink shadow-dialog data-[swipe=cancel]:translate-x-0 data-[swipe=cancel]:transition-transform data-[swipe=end]:translate-x-(--radix-toast-swipe-end-x) data-[swipe=end]:opacity-0 data-[swipe=end]:transition-[translate,opacity] data-[swipe=move]:translate-x-(--radix-toast-swipe-move-x) motion-reduce:transition-none"
        >
          <SquareKanban
            aria-hidden="true"
            size={15}
            className="mt-2 shrink-0"
          />
          <Toast.Description className="min-w-0 flex-1 break-words text-body">
            {toast.text}
          </Toast.Description>
          <Toast.Close
            aria-label="Dismiss"
            className={cn(
              "rounded-s p-2 text-on-ink/70 hover:text-on-ink",
              focusRing,
            )}
          >
            <X aria-hidden="true" size={14} />
          </Toast.Close>
        </Toast.Root>
      ))}
      {createPortal(
        <Toast.Viewport
          label="Card moves ({hotkey})"
          className="fixed right-16 bottom-16 left-16 z-20 flex flex-col gap-8 outline-none sm:left-auto sm:w-toast"
        />,
        document.body,
      )}
    </Toast.Provider>
  );
}

/** The top bar's bell: the project's last card moves, newest first, and a toast for each batch of moves that arrives. */
export function NotificationBell({ slug }: { slug: string }) {
  const feed = useFeed(slug);
  const status = useLiveStatus();
  const now = useNow(status.state === "reconnecting", MINUTE_MS);
  const [open, setOpen] = useState(false);
  const [fresh, setFresh] = useState<ReadonlySet<string>>(new Set());
  const unread = unreadCount(feed);
  return (
    <>
      <RadixPopover.Root
        open={open}
        onOpenChange={(next) => {
          if (next) {
            setFresh(
              new Set(feed.filter((e) => e.unread).map((e) => entryKey(e))),
            );
            void notificationStore.refill(slug);
          }
          notificationStore.markRead(slug);
          setOpen(next);
        }}
      >
        <RadixPopover.Trigger asChild>
          <button
            type="button"
            aria-label={bellLabel(unread)}
            className={cn(
              "relative flex shrink-0 items-center rounded-m px-10 py-7 text-text-muted hover:bg-sunken max-lg:size-44 max-lg:justify-center max-lg:p-0",
              focusRing,
            )}
          >
            <Bell size={15} aria-hidden="true" />
            {unread > 0 ? (
              <span
                aria-hidden="true"
                className="absolute top-2 right-2 flex h-16 max-lg:top-4 max-lg:right-4 min-w-16 items-center justify-center rounded-chip bg-primary px-4 text-micro font-semibold text-on-primary"
              >
                {unread}
              </span>
            ) : null}
          </button>
        </RadixPopover.Trigger>
        <RadixPopover.Portal>
          <RadixPopover.Content
            align="end"
            sideOffset={6}
            collisionPadding={16}
            aria-label="Notifications"
            className="z-10 flex w-notifications max-w-(--width-popover-max) flex-col rounded-m border border-border bg-raised shadow-card outline-none"
          >
            <div className="flex items-center gap-8 border-b border-border-subtle px-16 py-12">
              <h2 className="flex-1 text-card font-semibold text-text">
                Card moves
              </h2>
              {feed.length > 0 ? (
                <button
                  type="button"
                  onClick={() => {
                    notificationStore.clearAll(slug);
                  }}
                  className={cn(
                    "rounded-s px-6 py-2 text-code text-primary-text hover:bg-sunken",
                    focusRing,
                  )}
                >
                  Clear all
                </button>
              ) : null}
            </div>
            {feed.length === 0 ? (
              <p className="px-16 py-14 text-body text-text-muted">
                No card moves
              </p>
            ) : (
              <ul className="py-4">
                {feed.map((entry) => (
                  <Entry
                    key={entryKey(entry)}
                    slug={slug}
                    entry={entry}
                    fresh={fresh.has(entryKey(entry))}
                    now={now}
                    onOpen={() => {
                      setOpen(false);
                    }}
                  />
                ))}
              </ul>
            )}
          </RadixPopover.Content>
        </RadixPopover.Portal>
      </RadixPopover.Root>
      <MoveToasts slug={slug} />
    </>
  );
}
