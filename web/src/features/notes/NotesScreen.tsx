import {
  useMutation,
  useQuery,
  useQueryClient,
  useSuspenseQuery,
} from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import {
  Bot,
  Check,
  Layers,
  MessageSquareReply,
  NotebookPen,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { keys } from "@/api/keys";
import { markNotesRead, type NoteView } from "@/api/notes";
import { olderNotListed } from "@/api/pages";
import { notesQuery, notesUnreadQuery, sessionQuery } from "@/api/queries";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import { buttonClasses } from "@/components/button/buttonStyles";
import { Menu } from "@/components/overlay/Menu";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { Skeleton } from "@/components/states/Skeleton";
import { formatAsOf } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { PersonName } from "@/people/PersonName";
import { MessageBody } from "@/features/messages/MessageBody";
import { filterLabel } from "@/features/messages/filters";
import {
  NO_AREA,
  areaLabel,
  filterNotes,
  newNoteIds,
  noteOptions,
  notesHref,
  readMark,
  replyHref,
  type NotesSearch,
} from "./notes";

export function NotesScreen({
  slug,
  search,
}: {
  slug: string;
  search: NotesSearch;
}) {
  const { data: listing, dataUpdatedAt } = useSuspenseQuery(notesQuery(slug));
  const unread = useQuery(notesUnreadQuery(slug));
  const session = useQuery(sessionQuery());
  const router = useRouter();
  const queryClient = useQueryClient();
  const notes = listing.items;
  // The notes that were new when the screen opened stay marked while the human reads them, though the read mark moves at once.
  const [fresh, setFresh] = useState<Set<string> | null>(null);
  if (fresh === null && unread.data !== undefined) {
    setFresh(newNoteIds(notes, unread.data));
  }
  const read = useMutation({
    mutationFn: (through: string) => markNotesRead(slug, through),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: keys.notes(slug) }),
  });
  const human = session.data?.human ?? null;
  const mark = human === null ? null : readMark(notes, unread.data ?? null);
  const { mutate } = read;
  // Each note is marked once, so a failed mark or a count that is not refetched yet sends no second request.
  const marked = useRef<string | null>(null);
  useEffect(() => {
    if (mark !== null && marked.current !== mark) {
      marked.current = mark;
      mutate(mark);
    }
  }, [mark, mutate]);

  const options = noteOptions(notes);
  const visible = filterNotes(notes, search);

  function go(next: NotesSearch) {
    void router.navigate({ href: notesHref(slug, next) });
  }

  return (
    <div className="flex flex-col gap-16 px-24 pt-20 pb-24">
      <header className="flex flex-col gap-4">
        <h1 className="text-screen leading-snug font-semibold text-text">
          Notes
        </h1>
        <p className="text-body text-text-muted">
          Decisions the main agent and the leads took without asking you. A note
          asks you nothing; to ask about a decision or reverse it, reply in the
          chat with its author.
        </p>
      </header>
      <div className="flex flex-wrap items-center gap-6">
        <Menu
          trigger={
            <Button icon={<Bot aria-hidden="true" size={15} />}>
              {filterLabel("Agent", search.agent)}
            </Button>
          }
          items={[
            {
              label: "All agents",
              icon: search.agent === undefined ? Check : undefined,
              onSelect: () => {
                go({ ...search, note: undefined, agent: undefined });
              },
            },
            ...options.agents.map((agent) => ({
              label: agent,
              icon: agent === search.agent ? Check : undefined,
              onSelect: () => {
                go({ ...search, note: undefined, agent });
              },
            })),
          ]}
        />
        <Menu
          trigger={
            <Button icon={<Layers aria-hidden="true" size={15} />}>
              {areaLabel(search.area)}
            </Button>
          }
          items={[
            {
              label: "All areas",
              icon: search.area === undefined ? Check : undefined,
              onSelect: () => {
                go({ ...search, note: undefined, area: undefined });
              },
            },
            ...options.areas.map((area) => ({
              label: area,
              icon: area === search.area ? Check : undefined,
              onSelect: () => {
                go({ ...search, note: undefined, area });
              },
            })),
            ...(options.unscoped
              ? [
                  {
                    label: "No area",
                    icon: search.area === NO_AREA ? Check : undefined,
                    onSelect: () => {
                      go({ ...search, note: undefined, area: NO_AREA });
                    },
                  },
                ]
              : []),
          ]}
        />
      </div>
      {visible.length === 0 ? (
        <EmptyPanel
          icon={NotebookPen}
          title={notes.length === 0 ? "No notes yet" : "No note matches"}
        >
          {notes.length === 0
            ? "When the main agent or a lead decides something without asking you, it records the decision here."
            : "Change or clear the filters to see more notes."}
        </EmptyPanel>
      ) : (
        <ol aria-label="Notes" className="flex flex-col gap-12">
          {visible.map((note) => (
            <NoteCard
              key={note.id}
              slug={slug}
              note={note}
              now={dataUpdatedAt}
              isNew={fresh?.has(note.id) ?? false}
              current={note.id === search.note}
            />
          ))}
        </ol>
      )}
      {listing.more ? (
        <p className="text-bar text-text-muted">
          {olderNotListed(notes.length, "notes")}
        </p>
      ) : null}
    </div>
  );
}

function NoteCard({
  slug,
  note,
  now,
  isNew,
  current,
}: {
  slug: string;
  note: NoteView;
  now: number;
  isNew: boolean;
  current: boolean;
}) {
  return (
    <li
      aria-label={`${note.id} ${note.title}`}
      aria-current={current ? "true" : undefined}
      className={cn(
        "flex flex-col gap-8 rounded-l bg-surface px-16 py-14 inset-ring",
        current ? "inset-ring-primary" : "inset-ring-border-subtle",
      )}
    >
      <div className="flex flex-wrap items-center gap-8">
        <h2 className="text-card leading-snug font-semibold text-text">
          {note.title}
        </h2>
        <span className="font-mono text-meta text-text-faint">{note.id}</span>
        {isNew ? (
          <span className="rounded-s bg-ink px-6 py-2 text-caption leading-snug font-semibold text-on-ink">
            New
          </span>
        ) : null}
        <HrefLink
          href={replyHref(slug, note)}
          className={cn(buttonClasses("secondary", "sm"), focusRing, "ml-auto")}
        >
          <MessageSquareReply aria-hidden="true" size={13} />
          Reply
        </HrefLink>
      </div>
      <p className="flex flex-wrap items-center gap-8 text-meta text-text-muted">
        <PersonName name={note.author} slot="messageHead" />
        <span>{note.area === null ? "No area" : `Area ${note.area}`}</span>
        <span className="text-text-faint">
          {formatAsOf(now, note.written_at)}
        </span>
      </p>
      <MessageBody body={note.decision} slug={slug} />
      <div className="flex flex-col gap-2">
        <span className="text-meta font-medium text-text-muted">Why</span>
        <MessageBody body={note.reason} slug={slug} />
      </div>
      {note.links.length === 0 ? null : (
        <div className="flex flex-col gap-2">
          <span className="text-meta font-medium text-text-muted">About</span>
          <MessageBody body={note.links.join("  ")} slug={slug} />
        </div>
      )}
    </li>
  );
}

export function NotesPending() {
  return (
    <div className="flex flex-col gap-16 px-24 pt-20 pb-24">
      <Skeleton />
      <Skeleton />
    </div>
  );
}
