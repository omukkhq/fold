import { zodResolver } from "@hookform/resolvers/zod";
import { useQuery } from "@tanstack/react-query";
import { ArrowRight, MessageCircleQuestion, Send } from "lucide-react";
import { useState } from "react";
import { Controller, useForm } from "react-hook-form";
import type { ProposalDetail } from "@/api/proposals";
import { proposalFileDiffQuery } from "@/api/queries";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
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
import { questionSchema, type QuestionValues } from "./compose";
import {
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
} from "./conversation";
import { NAME_NOTE } from "./lineQuestions";
import { commentNote } from "./threadsModel";
import { detailHref } from "./view";

const OVERLINE =
  "text-overline leading-snug font-semibold text-text-muted uppercase";
const CARD = "rounded-l bg-surface inset-ring inset-ring-border-subtle";
const LINK = cn("rounded-s text-primary-text hover:underline", focusRing);

const KIND_CLASSES: Record<ConversationKind, string> = {
  to_author: "bg-primary-soft text-primary-text",
  to_you: "bg-secondary-soft text-secondary-text",
  review: "bg-sunken text-text",
  agent: "bg-sunken text-text-muted",
};

const STATE_CLASSES: Record<ConversationState, string> = {
  needs_answer: "bg-secondary-soft text-secondary-text",
  waits_author: "bg-primary-soft text-primary-text",
  open: "bg-secondary-soft text-secondary-text",
  closed: "bg-sunken text-text-muted",
};

function FilterGroup<T>({
  title,
  options,
  onPick,
}: {
  title: string;
  options: FilterOption<T>[];
  onPick: (value: T) => void;
}) {
  return (
    <section aria-label={title} className="flex flex-col gap-2">
      <h2 className={cn(OVERLINE, "px-10 pb-4")}>{title}</h2>
      {options.map((option) => (
        <button
          key={option.label}
          type="button"
          aria-pressed={option.selected}
          onClick={() => {
            onPick(option.value);
          }}
          className={cn(
            "flex items-center gap-8 rounded-m px-10 py-6 text-left text-bar",
            option.selected
              ? "bg-sunken font-semibold text-text"
              : "text-text-muted hover:bg-sunken hover:text-text",
            focusRing,
          )}
        >
          <span className="min-w-0 flex-1">{option.label}</span>
          <span className="rounded-s bg-sunken px-6 text-caption font-medium text-text-muted">
            {option.count}
          </span>
        </button>
      ))}
    </section>
  );
}

/** A question to the author about the whole proposal; the author decides on its own whether it needs a human's answer. */
function AskWholeProposal({
  slug,
  human,
  changesHref,
  onAsk,
}: {
  slug: string;
  human: string | null;
  changesHref: string;
  onAsk: (body: string) => Promise<void>;
}) {
  const {
    control,
    handleSubmit,
    reset,
    setError,
    formState: { errors, isSubmitting },
  } = useForm<QuestionValues>({
    resolver: zodResolver(questionSchema),
    defaultValues: { body: "" },
  });
  const submit = handleSubmit(async ({ body }) => {
    try {
      await onAsk(body.trim());
      reset();
    } catch (error) {
      setError("root", { message: failureText(error) });
    }
  });
  return (
    <form
      aria-label="Ask the author"
      noValidate
      onSubmit={(event) => void submit(event)}
      className={cn(CARD, "flex flex-col gap-10 p-14")}
    >
      <p className={cn(OVERLINE, "flex items-center gap-6")}>
        <MessageCircleQuestion aria-hidden="true" size={13} />
        Ask the author about the whole proposal
      </p>
      {human === null ? (
        <p className="text-code text-text-muted">{NAME_NOTE}</p>
      ) : (
        <>
          <Controller
            control={control}
            name="body"
            render={({ field }) => (
              <MessageComposer
                slug={slug}
                aria-label="Question to the author"
                placeholder="What do you want to know? This changes nothing in the review."
                pending={isSubmitting}
                aria-invalid={errors.body ? true : undefined}
                ref={field.ref}
                value={field.value}
                onValueChange={field.onChange}
                onBlur={field.onBlur}
              />
            )}
          />
          {errors.body ? (
            <p className="text-meta font-medium text-danger">
              {errors.body.message}
            </p>
          ) : null}
          {errors.root ? (
            <p role="alert" className="text-meta font-medium text-danger">
              The question was not sent. {errors.root.message}
            </p>
          ) : null}
          <div className="flex flex-wrap items-center gap-10">
            <span className="text-meta text-text-faint">
              To ask about some lines, select them in{" "}
              <HrefLink href={changesHref} className={LINK}>
                Changes
              </HrefLink>
              .
            </span>
            <Button
              type="submit"
              variant="primary"
              className="ml-auto"
              pending={isSubmitting}
              pendingLabel="Asking…"
              icon={<Send aria-hidden="true" size={13} />}
            >
              Ask the author
            </Button>
          </div>
        </>
      )}
    </form>
  );
}

function ReplyForm({
  slug,
  label,
  onSend,
}: {
  slug: string;
  label: string;
  onSend: (body: string) => Promise<void>;
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
            value={body}
            pending={sending}
            onValueChange={setBody}
          />
        </div>
        <Button
          type="submit"
          size="sm"
          pending={sending}
          pendingLabel="Sending…"
          icon={<Send aria-hidden="true" size={13} />}
        >
          Send
        </Button>
      </div>
      {failure ? (
        <p role="alert" className="text-meta font-medium text-danger">
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
        "border-b border-border-subtle bg-bg py-6 font-mono text-code leading-snug text-text",
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
            className="w-diff-number shrink-0 pr-8 text-right text-text-faint select-none"
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
      className={cn(CARD, "flex flex-col overflow-hidden")}
    >
      <header className="flex flex-wrap items-center gap-x-10 gap-y-6 border-b border-border-subtle px-16 py-10">
        <span
          className={cn(
            "rounded-s px-8 py-2 text-caption font-semibold",
            KIND_CLASSES[thread.kind],
          )}
        >
          {KIND_TEXT[thread.kind]}
        </span>
        <span className="min-w-0 font-mono text-meta break-all text-text">
          {whereText(thread)}
        </span>
        {version === null ? null : (
          <span className="text-meta text-text-faint">{version}</span>
        )}
        <span className="flex flex-wrap items-center gap-x-10 gap-y-4 md:ml-auto">
          <span
            className={cn(
              "rounded-s px-8 py-2 text-caption font-semibold",
              STATE_CLASSES[thread.state],
            )}
          >
            {thread.state === "waits_author" && thread.waitsFor.length > 0 ? (
              <>
                Waits for <PersonList names={thread.waitsFor} />
              </>
            ) : (
              thread.stateText
            )}
          </span>
          {diffHref === null ? null : (
            <HrefLink href={diffHref} className={cn("text-code", LINK)}>
              Show in diff
            </HrefLink>
          )}
          {source.type === "message" ? (
            <HrefLink
              href={idHref(slug, "message", source.thread.id)}
              className={cn("text-code", LINK)}
            >
              Open in Messages
            </HrefLink>
          ) : null}
        </span>
      </header>
      {source.type === "message" ? (
        <>
          <ol className="flex flex-col px-16">
            {source.thread.messages.map((message) =>
              message.sender === "fold" ? (
                <li
                  key={message.id}
                  className="border-b border-border-subtle py-8 text-meta text-text-faint italic last:border-b-0"
                >
                  <MentionText text={message.body} slug={slug} />
                </li>
              ) : (
                <li
                  key={message.id}
                  className="flex flex-col gap-6 border-b border-border-subtle py-12 last:border-b-0"
                >
                  <p className="flex flex-wrap items-center gap-x-8 text-code text-text-muted">
                    <PersonName
                      name={message.sender}
                      className="font-semibold"
                    />
                    <ArrowRight
                      aria-label="to"
                      role="img"
                      size={12}
                      className="shrink-0 text-text-faint"
                    />
                    <PersonList names={recipientActors(message)} />
                    <span className="text-text-faint">
                      · {formatAsOfMinute(now, message.sent_at)}
                    </span>
                  </p>
                  {message.lines === null ? null : (
                    <QuotedLines
                      slug={slug}
                      proposal={message.proposal}
                      lines={message.lines}
                    />
                  )}
                  <MessageBody body={message.body} slug={slug} />
                </li>
              ),
            )}
          </ol>
          <div className="flex flex-col gap-8 border-t border-border-subtle px-16 py-10">
            {threadCanMarkAnswered(source.thread) ? (
              <MarkAnswered slug={slug} scope={{ thread: source.thread.id }} />
            ) : null}
            {human === null ? null : (
              <ReplyForm
                slug={slug}
                label={`Reply in ${source.thread.id}`}
                onSend={async (body) => {
                  const last = source.thread.messages.at(-1);
                  if (last) await onReply(last.id, body);
                }}
              />
            )}
          </div>
        </>
      ) : (
        <>
          <CommentLines slug={slug} proposal={proposal} thread={thread} />
          <ol className="flex flex-col px-16">
            {source.thread.comments.map((entry) => (
              <li
                key={entry.comment.id}
                className="flex flex-col gap-4 border-b border-border-subtle py-12 last:border-b-0"
              >
                <p className="flex flex-wrap items-center gap-x-8 text-code text-text-muted">
                  <PersonName
                    name={entry.comment.author}
                    className="font-semibold"
                  />
                  <span className="text-text-faint">
                    · {commentNote(source.thread, entry)} ·{" "}
                    {formatAsOfMinute(now, entry.comment.created_at)}
                  </span>
                </p>
                <MentionText text={entry.comment.body} slug={slug} />
              </li>
            ))}
          </ol>
          {human !== null && proposal.summary.state === "open" ? (
            <div className="border-t border-border-subtle px-16 py-10">
              <ReplyForm
                slug={slug}
                label={`Reply to ${source.thread.id}`}
                onSend={async (body) => {
                  const last = source.thread.comments.at(-1)?.comment;
                  if (last) await onComment(last.id, body);
                }}
              />
            </div>
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
  changesHref: string;
  onAsk: (body: string) => Promise<void>;
  onReply: (replyTo: string, body: string) => Promise<void>;
  onComment: (replyTo: string, body: string) => Promise<void>;
}

export function ConversationTab({
  slug,
  proposal,
  threads,
  olderNote,
  human,
  now,
  changesHref,
  onAsk,
  onReply,
  onComment,
}: ConversationTabProps) {
  const [filter, setFilter] = useState<ConversationFilter>(NO_FILTER);
  const current = proposal.summary.current_version;
  const all = threads ?? [];
  const shown = filterConversation(all, filter);
  return (
    <div className="flex min-w-0 flex-col gap-24 px-16 py-20 md:px-24 lg:flex-row lg:items-start">
      <aside
        aria-label="Filters"
        className="flex min-w-0 flex-col gap-18 lg:w-filter-column lg:shrink-0"
      >
        <FilterGroup
          title="Show"
          options={stateOptions(all, filter)}
          onPick={(state) => {
            setFilter({ ...filter, state });
          }}
        />
        <FilterGroup
          title="Kind"
          options={kindOptions(all, filter)}
          onPick={(kind) => {
            setFilter({ ...filter, kind: filter.kind === kind ? null : kind });
          }}
        />
        <FilterGroup
          title="Version"
          options={versionOptions(all, filter, current)}
          onPick={(version) => {
            setFilter({ ...filter, version });
          }}
        />
      </aside>
      <div className="flex min-w-0 flex-1 flex-col gap-16">
        <AskWholeProposal
          slug={slug}
          human={human}
          changesHref={changesHref}
          onAsk={onAsk}
        />
        {threads === null ? (
          <Skeleton />
        ) : shown.length === 0 ? (
          <EmptyPanel title="No threads">
            {all.length === 0
              ? "Nobody wrote about this proposal yet."
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
          <p className="text-meta text-text-muted">{olderNote}</p>
        )}
      </div>
    </div>
  );
}
