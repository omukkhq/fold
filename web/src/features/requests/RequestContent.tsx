import { ActorText } from "@/people/ActorText";
import { PersonList, PersonName } from "@/people/PersonName";
import {
  ArrowRight,
  ChevronRight,
  Download,
  FileText,
  ImageIcon,
  Send,
} from "lucide-react";
import type { ReactNode } from "react";
import { useForm, Controller } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import {
  attachmentHref,
  type RequestAttachment,
  type RequestComment,
  type RequestDetail,
  type RequestDraft,
  type RequestMessage,
} from "@/api/requests";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import { recipientActors } from "@/features/messages/delivery";
import { Markdown } from "@/features/task/Markdown";
import { formatAsOfMinute } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { MentionText } from "@/mentions/MentionText";
import { MentionTextarea } from "@/mentions/MentionTextarea";
import type { Known } from "@/mentions/parse";
import { messageSchema, type MessageValues } from "./compose";
import type { Failure } from "./errors";
import {
  authorLine,
  commentsSummary,
  edgeLines,
  edgesSummary,
  formatSize,
  headingOf,
  kindOf,
  metaLine,
  threadSummary,
  versionText,
} from "./present";
import { KindBadge, StatePill } from "./RequestPills";

export const OVERLINE =
  "text-overline leading-snug font-semibold tracking-overline text-primary-text uppercase";

export function Crumb({ slug, id }: { slug: string; id: string }) {
  return (
    <nav
      aria-label="Breadcrumb"
      className="flex items-center gap-8 text-bar text-text-muted"
    >
      <HrefLink
        href={`/p/${slug}/requests`}
        className={cn("rounded-s hover:text-text", focusRing)}
      >
        Requests
      </HrefLink>
      <ChevronRight aria-hidden="true" size={13} />
      <span className="font-mono text-code text-text">{id}</span>
    </nav>
  );
}

export function RequestHeader({
  detail,
  now,
}: {
  detail: RequestDetail;
  now: number;
}) {
  const heading = headingOf(detail.request);
  return (
    <header className="flex flex-col gap-8">
      <h1
        className={cn(
          "text-page leading-snug font-semibold",
          heading.muted ? "text-text-muted" : "text-text",
        )}
      >
        {heading.text}
      </h1>
      <div className="flex flex-wrap items-center gap-8">
        <StatePill state={detail.request.state} />
        <KindBadge kind={kindOf(detail.request)} />
        <p className="text-bar text-text-muted">
          <ActorText
            segments={metaLine(detail, now)}
            personClassName="text-body"
          />
        </p>
      </div>
    </header>
  );
}

function Section({
  title,
  note,
  aside,
  children,
}: {
  title: string;
  note?: string;
  aside?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section aria-label={title} className="flex flex-col gap-10">
      <div className="flex items-center gap-10">
        <h2 className="text-section leading-snug font-semibold text-text">
          {title}
        </h2>
        {note ? (
          <p className="min-w-0 flex-1 text-meta text-text-muted">{note}</p>
        ) : (
          <span className="flex-1" />
        )}
        {aside}
      </div>
      {children}
    </section>
  );
}

function AttachmentRow({
  slug,
  id,
  attachment,
}: {
  slug: string;
  id: string;
  attachment: RequestAttachment;
}) {
  const Icon = attachment.media_type.startsWith("image/")
    ? ImageIcon
    : FileText;
  return (
    <li className="flex items-center gap-10 rounded-m bg-surface px-12 py-8 inset-ring inset-ring-border-subtle">
      <Icon aria-hidden="true" size={15} className="shrink-0 text-text-muted" />
      <span className="min-w-0 flex-1 truncate text-body text-text">
        {attachment.name}
      </span>
      <span className="font-mono text-meta text-text-muted">
        {formatSize(attachment.size)}
      </span>
      <a
        href={attachmentHref(slug, id, attachment.position)}
        download={attachment.name}
        aria-label={`Download ${attachment.name}`}
        className={cn(
          "inline-flex items-center gap-4 rounded-s text-meta text-text-muted hover:text-text",
          focusRing,
        )}
      >
        <Download aria-hidden="true" size={13} />
        Download
      </a>
    </li>
  );
}

export function BodySection({
  slug,
  detail,
  known,
}: {
  slug: string;
  detail: RequestDetail;
  known: Known;
}) {
  const id = detail.request.id;
  return (
    <Section title="Request">
      <Markdown mentions={{ slug, known }}>{detail.body}</Markdown>
      {detail.attachments.length > 0 ? (
        <div className="flex flex-col gap-6">
          <h3 className={OVERLINE}>Attachments</h3>
          <ul aria-label="Attachments" className="flex flex-col gap-6">
            {detail.attachments.map((a) => (
              <AttachmentRow
                key={a.position}
                slug={slug}
                id={id}
                attachment={a}
              />
            ))}
          </ul>
        </div>
      ) : null}
    </Section>
  );
}

export function VersionTabs({
  drafts,
  shown,
  disabled,
  onShow,
}: {
  drafts: readonly RequestDraft[];
  shown: number;
  disabled: boolean;
  onShow: (version: number) => void;
}) {
  return (
    <div
      role="group"
      aria-label="Draft version"
      className="flex gap-2 rounded-m bg-sunken p-3"
    >
      {drafts.map((d) => {
        const current = d.version === shown;
        return (
          <button
            key={d.version}
            type="button"
            aria-pressed={current}
            disabled={disabled}
            onClick={() => {
              onShow(d.version);
            }}
            className={cn(
              "rounded-s px-12 py-3 font-mono text-code disabled:opacity-60",
              current
                ? "bg-surface font-semibold text-text inset-ring inset-ring-border-subtle"
                : "text-text-muted hover:text-text",
              focusRing,
            )}
          >
            v{d.version}
          </button>
        );
      })}
    </div>
  );
}

export function DraftView({
  slug,
  detail,
  draft,
  known,
  now,
  tabs,
}: {
  slug: string;
  detail: RequestDetail;
  draft: RequestDraft | null;
  known: Known;
  now: number;
  tabs: ReactNode;
}) {
  if (draft === null) {
    return (
      <Section title="Draft">
        <p className="rounded-m bg-surface px-14 py-12 text-body text-text-muted inset-ring inset-ring-border-subtle">
          {detail.request.drafter === null ? (
            "No orchestrator has claimed the request yet. The cards appear here once one drafts them."
          ) : (
            <>
              <PersonName name={detail.request.drafter} /> is drafting the
              cards. They appear here when the draft is submitted.
            </>
          )}
        </p>
      </Section>
    );
  }
  const latest = detail.request.latest_version ?? draft.version;
  return (
    <>
      <Section
        title="Draft"
        note={versionText(draft.version, latest, detail.request.state)}
        aside={tabs}
      >
        <p className="flex items-center gap-8 text-meta text-text-muted">
          <PersonName name={draft.author} className="text-body" />
          {authorLine(draft, now)}
        </p>
        <ol className="flex flex-col gap-10">
          {draft.cards.map((card) => (
            <li
              key={card.key}
              className="flex flex-col gap-8 rounded-m bg-surface px-14 py-12 inset-ring inset-ring-border-subtle"
            >
              <p className="flex items-center gap-10">
                <span className="rounded-s bg-sunken px-6 py-2 font-mono text-meta text-text-muted">
                  {card.key}
                </span>
                <span className="text-card leading-snug font-semibold text-text">
                  {card.title}
                </span>
              </p>
              <Markdown mentions={{ slug, known }}>{card.description}</Markdown>
            </li>
          ))}
        </ol>
      </Section>
      <Section title="Dependencies" note={edgesSummary(draft.edges.length)}>
        {draft.edges.length > 0 ? (
          <ul aria-label="Dependencies" className="flex flex-col gap-8">
            {edgeLines(draft.edges, draft.cards).map((edge) => (
              <li
                key={`${edge.dependent}>${edge.prerequisite}`}
                className="flex items-baseline gap-8"
              >
                <span className="font-mono text-code text-text">
                  {edge.dependent}
                </span>
                <span className="text-meta text-text-muted">needs</span>
                <span className="font-mono text-code text-text">
                  {edge.prerequisite}
                </span>
                <span className="text-meta text-text-muted">{edge.text}</span>
              </li>
            ))}
          </ul>
        ) : null}
      </Section>
    </>
  );
}

function CommentCard({
  slug,
  comment,
  known,
  now,
}: {
  slug: string;
  comment: RequestComment;
  known: Known;
  now: number;
}) {
  return (
    <li className="flex flex-col gap-6 rounded-m bg-surface px-12 py-10 inset-ring inset-ring-border-subtle">
      <p className="flex items-center gap-8 text-meta text-text-muted">
        <PersonName name={comment.author} className="text-bar" />
        asked for changes on v{comment.version}
        <span className="ml-auto text-text-faint">
          {formatAsOfMinute(now, comment.created_at)}
        </span>
      </p>
      <MentionText text={comment.body} slug={slug} known={known} />
    </li>
  );
}

export function CommentsSection({
  slug,
  detail,
  known,
  now,
}: {
  slug: string;
  detail: RequestDetail;
  known: Known;
  now: number;
}) {
  return (
    <Section
      title="Comments"
      note={commentsSummary(detail.comments, detail.drafts)}
    >
      {detail.comments.length > 0 ? (
        <ul aria-label="Comments" className="flex flex-col gap-8">
          {detail.comments.map((c, index) => (
            <CommentCard
              key={`${String(c.created_at)}:${String(index)}`}
              slug={slug}
              comment={c}
              known={known}
              now={now}
            />
          ))}
        </ul>
      ) : null}
    </Section>
  );
}

function MessageCard({
  slug,
  message,
  known,
  now,
}: {
  slug: string;
  message: RequestMessage;
  known: Known;
  now: number;
}) {
  return (
    <li className="flex flex-col gap-6 rounded-m bg-surface px-12 py-10 inset-ring inset-ring-border-subtle">
      <p className="flex items-center gap-8 font-mono text-caption text-text-muted">
        <PersonName name={message.sender} slot="messageHead" />
        <ArrowRight
          aria-label="to"
          role="img"
          size={12}
          className="shrink-0 text-text-faint"
        />
        <PersonList names={recipientActors(message)} nameClassName="text-bar" />
        <span className="ml-auto font-ui text-meta text-text-faint">
          {formatAsOfMinute(now, message.sent_at)}
        </span>
      </p>
      <MentionText text={message.body} slug={slug} known={known} />
    </li>
  );
}

export interface ThreadProps {
  slug: string;
  detail: RequestDetail;
  messages: readonly RequestMessage[] | undefined;
  known: Known;
  now: number;
  /** Who the placeholder names. */
  target: string;
  /** Why no one can write; null while the thread is open. */
  closed: string | null;
  /** Resolves once the daemon has stored the message; rejects with its error. */
  onSend: (body: string) => Promise<void>;
  failure: Failure | null;
}

export function ThreadSection({
  slug,
  detail,
  messages,
  known,
  now,
  target,
  closed,
  onSend,
  failure,
}: ThreadProps) {
  const id = detail.request.id;
  const {
    control,
    handleSubmit,
    reset,
    formState: { errors, isSubmitting },
  } = useForm<MessageValues>({
    resolver: zodResolver(messageSchema),
    defaultValues: { body: "" },
  });
  const submit = handleSubmit(async ({ body }) => {
    try {
      await onSend(body.trim());
      reset();
    } catch {
      // The screen shows the failure it was handed.
    }
  });
  return (
    <Section
      title="Clarifications"
      note={threadSummary(detail.thread?.messages ?? 0, id)}
    >
      {messages && messages.length > 0 ? (
        <ul aria-label="Clarification thread" className="flex flex-col gap-8">
          {messages.map((m) => (
            <MessageCard
              key={m.id}
              slug={slug}
              message={m}
              known={known}
              now={now}
            />
          ))}
        </ul>
      ) : null}
      <form
        noValidate
        onSubmit={(event) => void submit(event)}
        className="flex flex-col gap-6"
      >
        <div className="flex items-start gap-8">
          <Controller
            control={control}
            name="body"
            render={({ field }) => (
              <MentionTextarea
                slug={slug}
                rows={1}
                ref={field.ref}
                value={field.value}
                onValueChange={field.onChange}
                onBlur={field.onBlur}
                aria-label="Message"
                disabled={closed !== null}
                readOnly={isSubmitting}
                aria-invalid={errors.body ? true : undefined}
                placeholder={`Reply to ${target} about ${id}…`}
                className="resize-none py-9"
              />
            )}
          />
          <Button
            type="submit"
            variant="primary"
            pending={isSubmitting}
            disabled={closed !== null}
            icon={<Send aria-hidden="true" size={13} />}
          >
            Send
          </Button>
        </div>
        {errors.body ? (
          <p className="text-meta font-medium text-danger">
            {errors.body.message}
          </p>
        ) : null}
        {failure ? (
          <p role="alert" className="text-meta font-medium text-danger">
            The message was not sent. {failure.text}{" "}
            <span className="font-mono">code: {failure.code ?? "none"}</span>
          </p>
        ) : null}
        <p className="text-meta text-text-faint">
          {closed ??
            "A message never changes the state of the request. Only the actions on the right do."}
        </p>
      </form>
    </Section>
  );
}
