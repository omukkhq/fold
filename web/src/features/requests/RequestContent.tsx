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
import { buttonClasses } from "@/components/button/buttonStyles";
import { Id } from "@/components/id/Id";
import { IdText } from "@/components/text/IdText";
import { recipientActors } from "@/features/messages/delivery";
import { Markdown } from "@/features/task/Markdown";
import { formatAsOfMinute } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { MentionText } from "@/mentions/MentionText";
import { MessageComposer } from "@/mentions/MessageComposer";
import type { Known } from "@/mentions/parse";
import { messageSchema, type MessageValues } from "./compose";
import type { Failure } from "./errors";
import {
  authorLine,
  commentsSummary,
  edgeLines,
  edgesSummary,
  formatSize,
  kindOf,
  metaLine,
  pageTitle,
  threadSummary,
  versionText,
} from "./present";
import { KindLabel } from "./RequestPills";
import { projectPath } from "@/routes/paths";

export const OVERLINE =
  "font-title text-13 leading-tight font-semibold text-text-2";

export function Crumb({ slug, id }: { slug: string; id: string }) {
  return (
    <nav
      aria-label="Breadcrumb"
      className="flex items-center gap-8 text-14 text-text-2"
    >
      <HrefLink
        href={projectPath(slug, "requests")}
        className={cn("rounded-s hover:text-text", focusRing)}
      >
        Requests
      </HrefLink>
      <ChevronRight aria-hidden="true" size={14} />
      <Id className="font-medium text-text">{id}</Id>
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
  return (
    <header className="flex flex-col gap-6">
      <h1 className="max-w-measure font-title text-24 leading-tight font-semibold text-text md:text-28">
        <IdText text={pageTitle(detail.request)} />
      </h1>
      <p className="flex flex-wrap items-center gap-x-8 gap-y-4 text-14 text-text-2">
        <KindLabel kind={kindOf(detail.request)} />
        <span aria-hidden="true" className="max-sm:hidden">
          ·
        </span>
        <span>
          <ActorText
            segments={metaLine(detail, now)}
            personClassName="text-14"
          />
        </span>
      </p>
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
      <div className="flex flex-wrap items-baseline gap-x-12 gap-y-4">
        <h2 className="font-title text-18 leading-tight font-semibold text-text">
          {title}
        </h2>
        {note ? (
          <p className="min-w-0 flex-1 text-13 text-text-2">
            <IdText text={note} />
          </p>
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
    <li className="flex max-w-measure items-center gap-10 rounded-m bg-surface py-8 pr-8 pl-12">
      <Icon aria-hidden="true" size={16} className="shrink-0 text-text-2" />
      <span className="min-w-0 flex-1 truncate text-14 text-text">
        {attachment.name}
      </span>
      <span className="text-13 text-text-2 tabular-nums">
        {formatSize(attachment.size)}
      </span>
      <a
        href={attachmentHref(slug, id, attachment.position)}
        download={attachment.name}
        aria-label={`Download ${attachment.name}`}
        className={cn(buttonClasses("quiet", "sm"), focusRing)}
      >
        <Download aria-hidden="true" size={14} />
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
      className="flex gap-2 rounded-s bg-fill p-3"
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
              "rounded-s px-12 py-3 text-14 tabular-nums disabled:opacity-60",
              current
                ? "bg-surface font-semibold text-text"
                : "text-text-2 hover:text-text",
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
        <p className="max-w-measure rounded-m bg-fill px-14 py-12 text-15 text-text">
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
        <p className="text-14 text-text">
          <PersonName name={draft.author} />{" "}
          <span className="text-13 text-text-2">{authorLine(draft, now)}</span>
        </p>
        <ol className="flex flex-col gap-10">
          {draft.cards.map((card) => (
            <li
              key={card.key}
              className="flex flex-col gap-4 rounded-m bg-surface px-16 py-14"
            >
              <p className="flex items-baseline gap-10">
                <span className="rounded-s bg-fill px-7 py-2 text-13 font-semibold text-text-2 tabular-nums">
                  {card.key}
                </span>
                <span className="text-16 leading-tight font-semibold text-text">
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
          <ul aria-label="Dependencies" className="flex flex-col gap-6 text-14">
            {edgeLines(draft.edges, draft.cards).map((edge) => (
              <li
                key={`${edge.dependent}>${edge.prerequisite}`}
                className="flex items-baseline gap-8"
              >
                <span className="font-semibold text-text tabular-nums">
                  {edge.dependent}
                </span>
                <span className="text-text-2">needs</span>
                <span className="font-semibold text-text tabular-nums">
                  {edge.prerequisite}
                </span>
                <span className="text-text-2">· {edge.text}</span>
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
    <li className="flex max-w-measure flex-col gap-6 rounded-m bg-surface px-14 py-10">
      <p className="flex flex-wrap items-baseline gap-x-8 text-13 text-text-2">
        <PersonName name={comment.author} className="text-14" />
        asked for changes on v{comment.version}
        <span className="ml-auto text-text-2 tabular-nums">
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
    <li className="flex flex-col gap-2">
      <p className="flex flex-wrap items-baseline gap-x-8 text-13 text-text-2">
        <PersonName name={message.sender} className="text-14 font-semibold" />
        <ArrowRight
          aria-label="to"
          role="img"
          size={14}
          className="shrink-0 self-center text-text-2"
        />
        <PersonList names={recipientActors(message)} nameClassName="text-13" />
        <span className="ml-auto text-13 text-text-2 tabular-nums">
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
        <ul
          aria-label="Clarification thread"
          className="flex max-w-measure flex-col gap-14"
        >
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
        <div className="flex max-w-measure items-end gap-8 rounded-m bg-surface py-8 pr-8 pl-14 has-[textarea:focus-visible]:outline-2 has-[textarea:focus-visible]:outline-offset-2 has-[textarea:focus-visible]:outline-accent">
          <Controller
            control={control}
            name="body"
            render={({ field }) => (
              <MessageComposer
                slug={slug}
                ref={field.ref}
                value={field.value}
                onValueChange={field.onChange}
                onBlur={field.onBlur}
                aria-label="Message"
                disabled={closed !== null}
                pending={isSubmitting}
                aria-invalid={errors.body ? true : undefined}
                placeholder={`Reply to ${target} about ${id}…`}
                className="min-w-0 flex-1 bg-transparent px-0 py-6 leading-normal focus-visible:outline-0"
              />
            )}
          />
          <Button
            type="submit"
            pending={isSubmitting}
            pendingLabel="Sending…"
            disabled={closed !== null}
            icon={<Send aria-hidden="true" size={14} />}
          >
            Send
          </Button>
        </div>
        {errors.body ? (
          <p className="text-13 font-medium text-bad">{errors.body.message}</p>
        ) : null}
        {failure ? (
          <p role="alert" className="text-13 font-medium text-bad">
            The message was not sent. {failure.text}{" "}
            <span className="font-mono">code: {failure.code ?? "none"}</span>
          </p>
        ) : null}
        <p className="text-13 text-text-2">
          {closed ??
            "A message never changes the state of the request. Only the actions on the right do."}
        </p>
      </form>
    </Section>
  );
}
