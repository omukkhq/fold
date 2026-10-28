import { atName } from "@/people/names";
import { ActorText } from "@/people/ActorText";
import { PersonName } from "@/people/PersonName";
import { zodResolver } from "@hookform/resolvers/zod";
import {
  Check,
  Kanban,
  Lock,
  MessageSquareWarning,
  Pencil,
  Save,
  X,
} from "lucide-react";
import type { ReactNode } from "react";
import { Controller, useForm } from "react-hook-form";
import type { RequestDetail } from "@/api/requests";
import { HrefLink } from "@/app/HrefLink";
import { Chip } from "@/components/badge/Chip";
import { Button } from "@/components/button/Button";
import { Id } from "@/components/id/Id";
import { IdText } from "@/components/text/IdText";
import {
  buttonClasses,
  buttonIconClass,
} from "@/components/button/buttonStyles";
import { COLUMNS } from "@/features/board/columns";
import { formatAsOfMinute } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { MentionText } from "@/mentions/MentionText";
import { MentionTextarea } from "@/mentions/MentionTextarea";
import type { Known } from "@/mentions/parse";
import {
  OLDER_DRAFT_NOTE,
  decisionText,
  pendingText,
  versionsNote,
  viewerText,
  type RequestAction,
} from "./actions";
import { changesSchema, type ChangesValues } from "./compose";
import type { Failure } from "./errors";
import {
  drafterLine,
  stateChip,
  statusHeading,
  type CreatedTask,
} from "./present";
import { OVERLINE } from "./RequestContent";
import { projectPath } from "@/routes/paths";

/** A part of a side card, headed one level under the card; `quiet` leaves its name to screen readers, where the card's heading already says it. */
function Block({
  title,
  quiet = false,
  children,
}: {
  title: string;
  quiet?: boolean;
  children: ReactNode;
}) {
  return (
    <section aria-label={title} className="flex flex-col gap-8">
      <h3 className={quiet ? "sr-only" : OVERLINE}>{title}</h3>
      {children}
    </section>
  );
}

const CARD = "flex flex-col gap-14 rounded-m bg-surface px-18 pt-16 pb-18";

/** The request's state, what it waits for and who acts next, above the actions that state allows. */
export function StatusCard({
  detail,
  human,
  now,
  children,
}: {
  detail: RequestDetail;
  human: string | null;
  now: number;
  children: ReactNode;
}) {
  const chip = stateChip(detail.request, human);
  return (
    <section aria-label="Status" className={CARD}>
      <div className="flex flex-col items-start gap-6">
        <Chip tone={chip.tone}>{chip.label}</Chip>
        <h2 className="font-title text-18 leading-tight font-semibold text-text">
          <ActorText
            segments={statusHeading(detail, human, now)}
            personClassName="text-18"
          />
        </h2>
      </div>
      {children}
    </section>
  );
}

/** Who drafts the request and its versions. */
export function AboutCard({ children }: { children: ReactNode }) {
  return (
    <section aria-label="About this request" className={CARD}>
      <h2 className="font-title text-16 leading-tight font-semibold text-text">
        About this request
      </h2>
      {children}
    </section>
  );
}

function FailureLine({ failure }: { failure: Failure | null }) {
  if (failure === null) return null;
  return (
    <p
      role="alert"
      className="flex flex-col gap-2 text-13 font-medium text-bad"
    >
      {failure.text}
      {failure.code ? (
        <span className="font-mono font-regular">code: {failure.code}</span>
      ) : null}
    </p>
  );
}

export interface DecisionProps {
  slug: string;
  detail: RequestDetail;
  actions: readonly RequestAction[];
  pending: RequestAction | null;
  failure: Failure | null;
  /** The cards of the latest version, which an approval turns into tasks. */
  cards: number;
  /** The draft on screen is not the latest, so the decisions that name the latest draft are off. */
  olderDraft: boolean;
  onApprove: () => void;
  /** Resolves once the daemon stored the comment; rejects with its error. */
  onRequestChanges: (comment: string) => Promise<void>;
  onEdit: () => void;
  onClose: () => void;
}

export function DecisionPanel({
  slug,
  detail,
  actions,
  pending,
  failure,
  cards,
  olderDraft,
  onApprove,
  onRequestChanges,
  onEdit,
  onClose,
}: DecisionProps) {
  const r = detail.request;
  const v = `v${String(r.latest_version ?? 0)}`;
  const busy = pending !== null;
  const {
    control,
    handleSubmit,
    reset,
    formState: { errors },
  } = useForm<ChangesValues>({
    resolver: zodResolver(changesSchema),
    defaultValues: { comment: "" },
  });
  const submit = handleSubmit(async ({ comment }) => {
    try {
      await onRequestChanges(comment.trim());
      reset();
    } catch {
      // The panel shows the failure it was handed.
    }
  });
  const has = (a: RequestAction) => actions.includes(a);
  return (
    <Block title="Your decision" quiet>
      <p className="text-15 text-text">
        <IdText text={decisionText(r, actions, busy)} />
      </p>
      {olderDraft ? (
        <p className="text-13 text-text-2">{OLDER_DRAFT_NOTE}</p>
      ) : null}
      {has("approve") ? (
        <>
          <Button
            variant="primary"
            className="w-full py-9"
            pending={pending === "approve"}
            pendingLabel={`Approving ${v}…`}
            disabled={busy}
            icon={<Check aria-hidden="true" size={16} />}
            onClick={onApprove}
          >
            Approve {v}
          </Button>
          <p className="text-13 text-text-2">
            {pending === "approve"
              ? pendingText("approve", r, cards)
              : `Creates ${String(cards)} ${cards === 1 ? "task" : "tasks"} on the board.`}
          </p>
        </>
      ) : null}
      {has("request_changes") ? (
        <form
          noValidate
          onSubmit={(event) => void submit(event)}
          className="flex flex-col gap-6 pt-8"
        >
          <label htmlFor="request-changes" className={OVERLINE}>
            Comment for {v}
          </label>
          <Controller
            control={control}
            name="comment"
            render={({ field }) => (
              <MentionTextarea
                id="request-changes"
                slug={slug}
                rows={3}
                ref={field.ref}
                value={field.value}
                onValueChange={field.onChange}
                onBlur={field.onBlur}
                readOnly={busy}
                aria-invalid={errors.comment ? true : undefined}
                placeholder={`What should ${r.drafter === null ? "the drafter" : atName(r.drafter)} change?`}
              />
            )}
          />
          {errors.comment ? (
            <p className="text-13 font-medium text-bad">
              {errors.comment.message}
            </p>
          ) : null}
          <Button
            type="submit"
            className="w-full"
            pending={pending === "request_changes"}
            pendingLabel="Requesting changes…"
            disabled={busy}
            icon={<MessageSquareWarning aria-hidden="true" size={16} />}
          >
            Request changes
          </Button>
          {pending === "request_changes" ? (
            <p className="text-13 text-text-2">
              {pendingText("request_changes", r, cards)}
            </p>
          ) : null}
        </form>
      ) : null}
      <div className="flex gap-8 pt-4">
        {has("edit") ? (
          <Button
            className="flex-1"
            disabled={busy}
            icon={<Pencil aria-hidden="true" size={16} />}
            onClick={onEdit}
          >
            Edit
          </Button>
        ) : null}
        {has("close") ? (
          <Button
            variant="danger"
            className="flex-1"
            pending={pending === "close"}
            pendingLabel="Closing…"
            disabled={busy}
            icon={<X aria-hidden="true" size={16} />}
            onClick={onClose}
          >
            Close
          </Button>
        ) : null}
      </div>
      {pending === "close" ? (
        <p className="text-13 text-text-2">{pendingText("close", r, cards)}</p>
      ) : null}
      <FailureLine failure={failure} />
    </Block>
  );
}

export function ViewerPanel({ detail }: { detail: RequestDetail }) {
  return (
    <Block title="Decision" quiet>
      <p className="flex items-start gap-8 rounded-s bg-fill px-12 py-10 text-14 text-text">
        <Lock
          aria-hidden="true"
          size={16}
          className="mt-2 shrink-0 text-text-2"
        />
        <ActorText segments={viewerText(detail.request)} />
      </p>
    </Block>
  );
}

export interface EditingProps {
  slug: string;
  detail: RequestDetail;
  note: string;
  saving: boolean;
  /** The first problem of the draft, or the daemon's refusal. */
  failure: Failure | null;
  onNote: (note: string) => void;
  onSave: () => void;
  onCancel: () => void;
}

export function EditingPanel({
  slug,
  detail,
  note,
  saving,
  failure,
  onNote,
  onSave,
  onCancel,
}: EditingProps) {
  const latest = detail.request.latest_version ?? 0;
  const next = `v${String(latest + 1)}`;
  return (
    <>
      <Block title="Editing">
        <p className="text-14 text-text">
          Edit the cards and the edges, then save. fold keeps v{latest} and adds
          your changes as {next}, which starts unapproved.
        </p>
        <label htmlFor="draft-note" className={OVERLINE}>
          Note for {next}
        </label>
        <MentionTextarea
          id="draft-note"
          slug={slug}
          rows={3}
          value={note}
          readOnly={saving}
          onValueChange={onNote}
          placeholder="Why did you change the draft?"
        />
        <Button
          variant="primary"
          pending={saving}
          pendingLabel={`Saving ${next}…`}
          icon={<Save aria-hidden="true" size={16} />}
          onClick={onSave}
        >
          Save as {next}
        </Button>
        <Button
          disabled={saving}
          icon={<X aria-hidden="true" size={16} />}
          onClick={onCancel}
        >
          Cancel
        </Button>
        {saving ? (
          <p className="text-13 text-text-2">
            Waiting for fold to save {next}. The cards and edges stay locked
            until it answers.
          </p>
        ) : null}
        <FailureLine failure={failure} />
      </Block>
      <Block title="A draft needs">
        <ul className="flex list-disc flex-col gap-4 pl-16 text-13 text-text-2">
          <li>1 to 20 cards</li>
          <li>A title of up to 200 characters on every card</li>
          <li>A description on every card</li>
          <li>Edges between cards that exist</li>
          <li>No cycle: c1 cannot need c3 if c3 needs c1</li>
        </ul>
      </Block>
    </>
  );
}

export function CreatedTasksPanel({
  slug,
  tasks,
}: {
  slug: string;
  tasks: readonly CreatedTask[];
}) {
  return (
    <Block title="Created tasks">
      <p className="text-14 text-text">
        The cards are tasks now. Their columns are calculated like every other
        task.
      </p>
      <ul aria-label="Created tasks" className="flex flex-col gap-8">
        {tasks.map((t) => {
          const column = COLUMNS.find((c) => c.status === t.status);
          return (
            <li key={t.task}>
              <HrefLink
                href={projectPath(slug, "t", t.task)}
                className={cn(
                  "flex flex-col gap-4 rounded-s bg-fill px-12 py-8 hover:bg-line",
                  focusRing,
                )}
              >
                <span className="flex items-center gap-8">
                  <Id className="text-13 font-semibold text-text">{t.task}</Id>
                  {t.key ? (
                    <span className="text-13 text-text-2">from {t.key}</span>
                  ) : null}
                  {column ? (
                    <span className="ml-auto flex items-center gap-4 text-13 text-text-2">
                      <span
                        aria-hidden="true"
                        className={cn("size-6 rounded-full", column.dot)}
                      />
                      {column.name}
                    </span>
                  ) : null}
                </span>
                <span className="text-14 text-text">{t.title}</span>
              </HrefLink>
            </li>
          );
        })}
      </ul>
      <HrefLink
        href={projectPath(slug, "board")}
        className={cn(buttonClasses("quiet", "md"), focusRing)}
      >
        <span className={buttonIconClass("quiet")}>
          <Kanban aria-hidden="true" size={16} />
        </span>
        Open on the board
      </HrefLink>
    </Block>
  );
}

export function ClosedPanel({
  slug,
  detail,
  known,
}: {
  slug: string;
  detail: RequestDetail;
  known: Known;
}) {
  return (
    <Block title="Closed" quiet>
      <p className="text-14 text-text">
        <PersonName name={detail.request.requester} /> closed the request. It
        only reads now.
      </p>
      {detail.close_reason ? (
        <MentionText
          text={detail.close_reason}
          slug={slug}
          known={known}
          className="text-14"
        />
      ) : null}
    </Block>
  );
}

export function DrafterBlock({
  detail,
  now,
}: {
  detail: RequestDetail;
  now: number;
}) {
  const drafter = detail.request.drafter;
  return (
    <Block title="Drafter">
      {drafter === null ? (
        <p className="text-14 text-text-2">
          No orchestrator has claimed the request.
        </p>
      ) : (
        <>
          <p className="flex items-center gap-8">
            <PersonName name={drafter} className="text-15" />
          </p>
          <p className="text-13 text-text-2">
            {drafterLine(detail.drafts, drafter, now)}
          </p>
        </>
      )}
    </Block>
  );
}

export function VersionsBlock({
  detail,
  human,
  now,
}: {
  detail: RequestDetail;
  /** The signed-in human; the note differs for the requester. */
  human: string | null;
  now: number;
}) {
  if (detail.drafts.length === 0) return null;
  const latest = detail.request.latest_version;
  return (
    <Block title="Versions">
      <ul className="flex flex-col gap-6">
        {[...detail.drafts].reverse().map((d) => (
          <li key={d.version} className="flex items-baseline gap-8 text-14">
            <span
              className={cn(
                "tabular-nums",
                d.version === latest
                  ? "font-semibold text-text"
                  : "text-text-2",
              )}
            >
              v{d.version}
            </span>
            <span className="min-w-0 flex-1 truncate text-text-2">
              <PersonName name={d.author} className="text-14" />
            </span>
            <span className="text-13 text-text-2 tabular-nums">
              {d.version === latest ? "current · " : ""}
              {formatAsOfMinute(now, d.created_at)}
            </span>
          </li>
        ))}
      </ul>
      <p className="text-13 text-text-2">
        <ActorText segments={versionsNote(detail.request, human)} />
      </p>
    </Block>
  );
}
