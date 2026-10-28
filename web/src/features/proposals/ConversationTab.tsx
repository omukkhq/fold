import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { useQuery } from "@tanstack/react-query";
import {
  ArrowRight,
  Check,
  ChevronDown,
  MessagesSquare,
  Send,
} from "lucide-react";
import { useState, type ReactNode } from "react";
import type { ProposalDetail } from "@/api/proposals";
import { proposalFileDiffQuery } from "@/api/queries";
import { HrefLink } from "@/app/HrefLink";
import { Chip } from "@/components/badge/Chip";
import type { ChipTone } from "@/components/badge/chips";
import { Button } from "@/components/button/Button";
import { menuContent, menuItem } from "@/components/overlay/menuStyles";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { Skeleton } from "@/components/states/Skeleton";
import { CODE_BOX, textFlow } from "@/components/text/textFlow";
import { MarkAnswered } from "@/features/messages/MarkAnswered";
import { MessageBody } from "@/features/messages/MessageBody";
import { QuotedLines } from "@/features/messages/QuotedLines";
import { threadCanMarkAnswered } from "@/features/messages/answered";
import { recipientActors } from "@/features/messages/delivery";
import { failureText } from "@/format/failure";
import { formatAsOfMinute } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { MentionText } from "@/mentions/MentionText";
import { MessageComposer } from "@/mentions/MessageComposer";
import { PersonList, PersonName } from "@/people/PersonName";
import { idHref } from "@/routes/paths";
import {
  chipStates,
  chosenLabel,
  filterConversation,
  KIND_TEXT,
  kindOptions,
  laterChange,
  linesChanged,
  NO_FILTER,
  rangeLines,
  stateOptions,
  versionOptions,
  versionText,
  whereText,
  type ConversationFilter,
  type ConversationKind,
  type ConversationState,
  type ConversationThread,
  type FilterOption,
  type StateFilter,
} from "./conversation";
import { commentNote } from "./threadsModel";
import { detailHref } from "./view";

const CARD = "rounded-m bg-surface";
const LINK = cn("rounded-s text-accent hover:underline", focusRing);

const KIND_CLASSES: Record<ConversationKind, string> = {
  to_author: "text-text",
  to_you: "text-text",
  review: "text-text",
  agent: "text-text-2",
};

/** Gold only for a thread that waits for the human's answer. */
const STATE_TONES: Record<ConversationState, ChipTone> = {
  needs_answer: "warn",
  waits_author: "neutral",
  open: "neutral",
  closed: "neutral",
};

function FilterChips({
  options,
  onPick,
}: {
  options: FilterOption<StateFilter>[];
  onPick: (value: StateFilter) => void;
}) {
  return (
    <>
      {options.map((option) => (
        <button
          key={option.label}
          type="button"
          aria-pressed={option.selected}
          onClick={() => {
            onPick(option.value);
          }}
          className={cn(
            "inline-flex items-center gap-6 rounded-pill px-12 py-5 text-14 whitespace-nowrap",
            option.selected
              ? "bg-accent-soft font-semibold text-accent"
              : "bg-surface text-text hover:bg-fill",
            focusRing,
          )}
        >
          {option.label}
          <span
            className={cn(
              "text-13 tabular-nums",
              option.selected ? "text-accent" : "text-text-2",
            )}
          >
            {option.count}
          </span>
        </button>
      ))}
    </>
  );
}

/** A dropdown of the filter row: one radio item per option, so a screen reader hears which one is chosen. */
function FilterMenu<T>({
  label,
  none,
  options,
  onPick,
}: {
  /** Names the dropdown for a screen reader, such as `Kind`. */
  label: string;
  none: string;
  options: FilterOption<T>[];
  onPick: (value: T) => void;
}) {
  const shown = chosenLabel(options, none);
  const chosen = options.findIndex((option) => option.selected);
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          aria-label={`${label}: ${shown}`}
          className={cn(
            "inline-flex items-center gap-6 rounded-s bg-surface py-5 pr-10 pl-12 text-14 whitespace-nowrap text-text hover:bg-fill",
            focusRing,
          )}
        >
          {shown}
          <ChevronDown aria-hidden="true" size={14} className="text-text-2" />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="start"
          sideOffset={6}
          collisionPadding={16}
          className={menuContent}
        >
          <DropdownMenu.RadioGroup
            value={String(chosen)}
            onValueChange={(value) => {
              const option = options[Number(value)];
              if (option) onPick(option.value);
            }}
          >
            {options.map((option, index) => (
              <DropdownMenu.RadioItem
                key={option.label}
                value={String(index)}
                className={cn(menuItem, "gap-10")}
              >
                <span className="inline-flex size-16 shrink-0 items-center justify-center">
                  <DropdownMenu.ItemIndicator>
                    <Check aria-hidden="true" size={16} />
                  </DropdownMenu.ItemIndicator>
                </span>
                <span className="flex-1">{option.label}</span>
                <span className="text-13 text-text-2 tabular-nums">
                  {option.count}
                </span>
              </DropdownMenu.RadioItem>
            ))}
          </DropdownMenu.RadioGroup>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

/** A reply box that opens when the human chooses Reply, so a thread shows its messages first. */
function Reply({
  slug,
  label,
  onSend,
  children,
}: {
  slug: string;
  label: string;
  onSend: (body: string) => Promise<void>;
  /** Other actions of the thread, beside Reply. */
  children?: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  if (open) {
    return (
      <ReplyForm
        slug={slug}
        label={label}
        onSend={async (body) => {
          await onSend(body);
          setOpen(false);
        }}
        onCancel={() => {
          setOpen(false);
        }}
      />
    );
  }
  return (
    <div className="flex flex-wrap items-center gap-8">
      <Button
        size="sm"
        icon={<Send aria-hidden="true" size={14} />}
        onClick={() => {
          setOpen(true);
        }}
      >
        Reply
      </Button>
      {children}
    </div>
  );
}

function ReplyForm({
  slug,
  label,
  onSend,
  onCancel,
}: {
  slug: string;
  label: string;
  onSend: (body: string) => Promise<void>;
  onCancel: () => void;
}) {
  const [body, setBody] = useState("");
  const [sending, setSending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  return (
    <form
      aria-label={label}
      noValidate
      className="flex flex-col gap-6"
      onSubmit={(event) => {
        event.preventDefault();
        const text = body.trim();
        if (text === "") {
          setFailure("Write a reply first.");
          return;
        }
        setSending(true);
        setFailure(null);
        onSend(text)
          .then(() => {
            setBody("");
          })
          .catch((error: unknown) => {
            setFailure(`The reply was not sent. ${failureText(error)}`);
          })
          .finally(() => {
            setSending(false);
          });
      }}
    >
      <div className="flex flex-wrap items-end gap-8">
        <div className="min-w-0 flex-1">
          <MessageComposer
            slug={slug}
            aria-label="Reply"
            placeholder="Reply"
            autoFocus
            value={body}
            pending={sending}
            onValueChange={setBody}
          />
        </div>
        <Button variant="ghost" size="sm" disabled={sending} onClick={onCancel}>
          Cancel
        </Button>
        <Button
          type="submit"
          size="sm"
          pending={sending}
          pendingLabel="Sending…"
          icon={<Send aria-hidden="true" size={14} />}
        >
          Send
        </Button>
      </div>
      {failure ? (
        <p role="alert" className="text-13 font-medium text-bad">
          {failure}
        </p>
      ) : null}
    </form>
  );
}

function useLinesChanged(
  slug: string,
  proposal: ProposalDetail,
  thread: ConversationThread,
): boolean {
  const current = proposal.summary.current_version;
  const change = laterChange(thread, current);
  const query = useQuery({
    ...proposalFileDiffQuery(
      slug,
      proposal.summary.id,
      { version: current, since: change?.since ?? null },
      change?.path ?? "",
    ),
    enabled: change !== null,
  });
  return (
    change !== null &&
    thread.lines !== null &&
    query.data !== undefined &&
    linesChanged(query.data.hunks, thread.lines)
  );
}

/** The commented lines as the comment's version holds them, read from that version's change. */
function CommentLines({
  slug,
  proposal,
  thread,
}: {
  slug: string;
  proposal: ProposalDetail;
  thread: ConversationThread;
}) {
  const range = thread.lines;
  const query = useQuery({
    ...proposalFileDiffQuery(
      slug,
      proposal.summary.id,
      { version: thread.version ?? 1, since: null },
      range?.path ?? "",
    ),
    enabled: range !== null && thread.version !== null,
  });
  const lines =
    range === null || query.data === undefined
      ? []
      : rangeLines(query.data.hunks, range);
  if (lines.length === 0) return null;
  return (
    <pre
      className={cn(
        CODE_BOX,
        "rounded-s bg-sunken/60 py-6 font-mono text-13 leading-normal text-text",
      )}
    >
      {lines.map((line) => (
        <div
          key={`${line.kind}:${String(line.n)}`}
          className={cn(
            "flex",
            line.kind === "added" && "bg-diff-add",
            line.kind === "removed" && "bg-diff-del",
          )}
        >
          <span
            aria-hidden="true"
            className="w-diff-number shrink-0 pr-8 text-right text-text-2 select-none"
          >
            {line.n}
          </span>
          <code className={cn("pr-10", textFlow("code"))}>{line.text}</code>
        </div>
      ))}
    </pre>
  );
}

function ThreadCard({
  slug,
  proposal,
  thread,
  human,
  now,
  onReply,
  onComment,
}: {
  slug: string;
  proposal: ProposalDetail;
  thread: ConversationThread;
  human: string | null;
  now: number;
  onReply: (replyTo: string, body: string) => Promise<void>;
  onComment: (replyTo: string, body: string) => Promise<void>;
}) {
  const id = proposal.summary.id;
  const current = proposal.summary.current_version;
  const changed = useLinesChanged(slug, proposal, thread);
  const version = versionText(thread, current, changed);
  const source = thread.source;
  const diffHref =
    thread.lines === null || thread.version === null
      ? null
      : detailHref(slug, id, {
          tab: "changes",
          ...(thread.version === current ? {} : { version: thread.version }),
          path: thread.lines.path,
        });
  return (
    <article
      aria-label={`${KIND_TEXT[thread.kind]} ${thread.key}`}
      data-state={thread.state}
      className={cn(CARD, "flex flex-col gap-12 px-18 pt-14 pb-16")}
    >
      <header className="flex flex-wrap items-center gap-x-10 gap-y-6">
        <span
          className={cn(
            "font-title text-15 font-semibold",
            KIND_CLASSES[thread.kind],
          )}
        >
          {KIND_TEXT[thread.kind]}
        </span>
        <span className="min-w-0 font-mono text-13 break-all text-text-2">
          {whereText(thread)}
        </span>
        {version === null ? null : (
          <span className="text-13 text-text-2">{version}</span>
        )}
        <Chip tone={STATE_TONES[thread.state]}>
          {thread.state === "waits_author" && thread.waitsFor.length > 0 ? (
            <>
              Waits for <PersonList names={thread.waitsFor} />
            </>
          ) : (
            thread.stateText
          )}
        </Chip>
        <span className="flex flex-wrap items-center gap-x-14 gap-y-4 text-14 md:ml-auto">
          {diffHref === null ? null : (
            <HrefLink href={diffHref} className={LINK}>
              Show in diff
            </HrefLink>
          )}
          {source.type === "message" ? (
            <HrefLink
              href={idHref(slug, "message", source.thread.id)}
              className={LINK}
            >
              Open in Messages
            </HrefLink>
          ) : null}
        </span>
      </header>
      {source.type === "message" ? (
        <>
          <ol className="flex flex-col gap-10">
            {source.thread.messages.map((message) =>
              message.sender === "fold" ? (
                <li key={message.id} className="text-14 text-text-2 italic">
                  <MentionText text={message.body} slug={slug} />
                </li>
              ) : (
                <li key={message.id} className="flex flex-col gap-4">
                  <p className="flex flex-wrap items-center gap-x-6 text-13 text-text-2">
                    <PersonName name={message.sender} />
                    <ArrowRight
                      aria-label="to"
                      role="img"
                      size={14}
                      className="shrink-0 text-text-2"
                    />
                    <PersonList names={recipientActors(message)} />
                    <span>· {formatAsOfMinute(now, message.sent_at)}</span>
                  </p>
                  {message.lines === null ? null : (
                    <QuotedLines
                      slug={slug}
                      proposal={message.proposal}
                      lines={message.lines}
                    />
                  )}
                  <div className="max-w-measure text-15 leading-normal">
                    <MessageBody body={message.body} slug={slug} />
                  </div>
                </li>
              ),
            )}
          </ol>
          {human === null ? (
            threadCanMarkAnswered(source.thread) ? (
              <MarkAnswered slug={slug} scope={{ thread: source.thread.id }} />
            ) : null
          ) : (
            <Reply
              slug={slug}
              label={`Reply in ${source.thread.id}`}
              onSend={async (body) => {
                const last = source.thread.messages.at(-1);
                if (last) await onReply(last.id, body);
              }}
            >
              {threadCanMarkAnswered(source.thread) ? (
                <MarkAnswered
                  slug={slug}
                  scope={{ thread: source.thread.id }}
                />
              ) : null}
            </Reply>
          )}
        </>
      ) : (
        <>
          <CommentLines slug={slug} proposal={proposal} thread={thread} />
          <ol className="flex flex-col gap-10">
            {source.thread.comments.map((entry) => (
              <li
                key={entry.comment.id}
                className={cn(
                  "flex flex-col gap-4",
                  entry.reply && "border-l-3 border-line pl-16",
                )}
              >
                <p className="text-13 text-text-2">
                  <PersonName name={entry.comment.author} /> ·{" "}
                  {commentNote(source.thread, entry)} ·{" "}
                  {formatAsOfMinute(now, entry.comment.created_at)}
                </p>
                <MentionText
                  text={entry.comment.body}
                  slug={slug}
                  className="max-w-measure text-15 leading-normal"
                />
              </li>
            ))}
          </ol>
          {human !== null && proposal.summary.state === "open" ? (
            <Reply
              slug={slug}
              label={`Reply to ${source.thread.id}`}
              onSend={async (body) => {
                const last = source.thread.comments.at(-1)?.comment;
                if (last) await onComment(last.id, body);
              }}
            />
          ) : null}
        </>
      )}
    </article>
  );
}

export interface ConversationTabProps {
  slug: string;
  proposal: ProposalDetail;
  /** Null while the threads or the comments are read. */
  threads: readonly ConversationThread[] | null;
  /** The line that says older message threads were left out; null when every thread was read. */
  olderNote: string | null;
  human: string | null;
  now: number;
  onReply: (replyTo: string, body: string) => Promise<void>;
  onComment: (replyTo: string, body: string) => Promise<void>;
}

/** The threads about the proposal under one row of filters: a chip for each state that holds threads, and a dropdown each for the kind and the version. A question about the whole proposal starts from the decision bar. */
export function ConversationTab({
  slug,
  proposal,
  threads,
  olderNote,
  human,
  now,
  onReply,
  onComment,
}: ConversationTabProps) {
  const [filter, setFilter] = useState<ConversationFilter>(NO_FILTER);
  const current = proposal.summary.current_version;
  const all = threads ?? [];
  const shown = filterConversation(all, filter);
  return (
    <div className="flex min-w-0 max-w-thread-column flex-col gap-14 px-16 pt-18 pb-28 md:px-24">
      <aside aria-label="Filters" className="flex flex-wrap items-center gap-8">
        <FilterChips
          options={chipStates(stateOptions(all, filter))}
          onPick={(state) => {
            setFilter({ ...filter, state });
          }}
        />
        <span aria-hidden="true" className="mx-4 h-20 w-px bg-line" />
        <FilterMenu
          label="Kind"
          none="Any kind"
          options={kindOptions(all, filter)}
          onPick={(kind) => {
            setFilter({ ...filter, kind });
          }}
        />
        <FilterMenu
          label="Version"
          none="All versions"
          options={versionOptions(all, filter, current)}
          onPick={(version) => {
            setFilter({ ...filter, version });
          }}
        />
      </aside>
      {threads === null ? (
        <Skeleton />
      ) : shown.length === 0 ? (
        <EmptyPanel icon={MessagesSquare} title="No threads">
          {all.length === 0
            ? "Nobody wrote about this proposal yet. Ask the author from the bar above."
            : "No thread matches these filters."}
        </EmptyPanel>
      ) : (
        shown.map((thread) => (
          <ThreadCard
            key={thread.key}
            slug={slug}
            proposal={proposal}
            thread={thread}
            human={human}
            now={now}
            onReply={onReply}
            onComment={onComment}
          />
        ))
      )}
      {olderNote === null ? null : (
        <p className="text-13 text-text-2">{olderNote}</p>
      )}
    </div>
  );
}
