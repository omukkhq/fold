import { Id } from "@/components/id/Id";
import { zodResolver } from "@hookform/resolvers/zod";
import { useQuery } from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import {
  ChevronDown,
  ChevronUp,
  ClipboardCheck,
  Hand,
  Pencil,
  Plus,
  UserCheck,
  X,
} from "lucide-react";
import { useEffect, useState } from "react";
import { useForm } from "react-hook-form";
import { sessionQuery } from "@/api/queries";
import {
  addDependency,
  clearBlocker,
  clearHumanReview,
  completeTask,
  removeDependency,
  sendBackTask,
  setBlocker,
  setHumanReview,
  type LiveWorkspace,
  type TaskDetail,
} from "@/api/tasks";
import { HrefLink } from "@/app/HrefLink";
import { Chip } from "@/components/badge/Chip";
import { Button, IconButton } from "@/components/button/Button";
import { Textarea, TextField } from "@/components/form/controls";
import { CommitId } from "@/components/mono/CommitId";
import { Mono } from "@/components/mono/Mono";
import { textFlow } from "@/components/text/textFlow";
import { taskPath } from "@/features/board/card";
import { TaskCostSection } from "@/features/cost/TaskCostSection";
import { formatAge } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing, insetFocusRing } from "@/lib/focus";
import { ActorText } from "@/people/ActorText";
import { PersonName } from "@/people/PersonName";
import {
  canEditDependencies,
  groupDependencies,
  waitingText,
  type DependencyRow,
} from "./dependencies";
import {
  KIND_NOTE,
  leadCheck,
  needsProposal,
  reportedText,
  resolvedText,
} from "./leadCheck";
import { inMainArea, mainReviewNote } from "./areas";
import { blockerControls, blockerView } from "./humanTask";
import { useAreas } from "./LabelFields";
import { LabelsSide } from "./LabelsSide";
import { recordedCheckpoints } from "./liveStrip";
import { proposalPath, revisionPath } from "./links";
import {
  NothingYet,
  SideCard,
  SideGroup,
  SideNote,
  SmallAction,
  submit,
  WriteError,
} from "./parts";
import {
  blockerSchema,
  dependencySchema,
  humanReviewSchema,
  sendBackSchema,
  type BlockerForm as BlockerValues,
  type DependencyForm,
  type HumanReviewForm,
  type HumanReviewInput,
  type SendBackForm as SendBackValues,
} from "./schemas";
import { linkGaps, nothingText, peopleGaps, workGaps } from "./sideGroups";
import {
  checkpointText,
  claimNote,
  collapse,
  historyText,
  humanReviewLines,
  integrationText,
  proposalStateText,
  reviewText,
  statusLook,
} from "./text";
import { useTaskWrite } from "./useTaskWrite";

interface Props {
  slug: string;
  task: TaskDetail;
  /** The workspace frame from the event stream. */
  live: LiveWorkspace | null;
  now: number;
}

function MoreToggle({
  text,
  open,
  onToggle,
}: {
  text: string | null;
  open: boolean;
  onToggle: () => void;
}) {
  if (text === null) {
    return null;
  }
  return (
    <SmallAction
      icon={open ? ChevronUp : ChevronDown}
      aria-expanded={open}
      className="self-start text-13"
      onClick={onToggle}
    >
      {text}
    </SmallAction>
  );
}

/** Who holds the implementation claim; the People group names an empty claim in its Nothing yet line. */
function Responsible({ task, now }: { task: TaskDetail; now: number }) {
  // Every claim of a task read is its implementation claim.
  const implementation = task.claims[0];
  // The task read lists implementation claims only, so the review row draws for no task.
  const review = task.claims.find((c) => (c.kind as string) === "review");
  if (implementation === undefined) {
    return null;
  }
  return (
    <SideGroup title="Responsible">
      <PersonName name={implementation.holder} />
      <SideNote className={cn(implementation.stale && "text-warn")}>
        {claimNote(implementation, now)}
      </SideNote>
      {review ? (
        <SideNote>
          Review claim: <PersonName name={review.holder} /> ·{" "}
          {claimNote(review, now)}
        </SideNote>
      ) : null}
    </SideGroup>
  );
}

function Workspace({
  task,
  live,
}: {
  task: TaskDetail;
  live: LiveWorkspace | null;
}) {
  const ws = task.workspace_info;
  if (task.workspace === null) {
    return null;
  }
  const base = ws
    ? ws.base === ws.main
      ? `Base ${ws.base} (main)`
      : `Base ${ws.base} (main is ${ws.main})`
    : live
      ? `Base ${live.base}`
      : null;
  return (
    <SideGroup title="Workspace">
      <p className="flex flex-wrap items-baseline gap-x-6">
        <Id>{task.workspace}</Id>
        {ws ? (
          <>
            <span aria-hidden="true" className="text-text-2">
              ·
            </span>
            <Mono className="min-w-0 wrap-anywhere">{ws.directory}</Mono>
          </>
        ) : null}
      </p>
      {base === null ? null : <SideNote>{base}</SideNote>}
      {ws && ws.behind > 0 ? (
        <SideNote className="self-start rounded-s bg-bg px-9 py-5">
          Main moved {ws.behind} {ws.behind === 1 ? "revision" : "revisions"}{" "}
          since this base
        </SideNote>
      ) : null}
    </SideGroup>
  );
}

function Checkpoints({ task, live, now }: Props) {
  const [open, setOpen] = useState(false);
  const all = recordedCheckpoints(task.checkpoints, live);
  const list = collapse(all, 1, open);
  if (all.length === 0) {
    return null;
  }
  return (
    <SideGroup title="Checkpoints">
      <ul className="flex flex-col gap-4">
        {list.shown.map((ck) => (
          <li
            key={ck.id}
            className="flex flex-wrap items-baseline justify-between gap-x-8"
          >
            <span className="min-w-0 wrap-anywhere">
              <Id>{ck.id}</Id> · {checkpointText(ck)}
            </span>
            <span className="shrink-0 text-13 text-text-2">
              {formatAge(now, ck.created_at)}
            </span>
          </li>
        ))}
      </ul>
      <MoreToggle
        text={list.more}
        open={open}
        onToggle={() => {
          setOpen(!open);
        }}
      />
    </SideGroup>
  );
}

function Changesets({ task, now }: { task: TaskDetail; now: number }) {
  if (task.changesets.length === 0) {
    return null;
  }
  return (
    <SideGroup title="Changesets">
      <ul className="flex flex-col gap-4">
        {task.changesets.map((cs) => (
          <li
            key={cs.id}
            className="flex flex-wrap items-baseline justify-between gap-x-8"
          >
            <span className="min-w-0 wrap-anywhere">
              <Id>{cs.id}</Id> · {cs.message}
            </span>
            <span className="shrink-0 text-13 text-text-2">
              {formatAge(now, cs.created_at)}
            </span>
          </li>
        ))}
      </ul>
    </SideGroup>
  );
}

function Proposal({ slug, task }: { slug: string; task: TaskDetail }) {
  const { proposal, integration } = task;
  if (proposal === null && integration === null) {
    return null;
  }
  return (
    <SideGroup title="Proposal">
      {proposal ? (
        <>
          <p className="wrap-anywhere">
            <Id href={proposalPath(slug, proposal.id)}>{proposal.id}</Id> ·{" "}
            {proposalStateText(
              proposal.state,
              proposal.version,
              proposal.review_claim,
            )}
          </p>
          {proposal.reviews.length > 0 ? (
            <ul aria-label="Reviews" className="flex flex-col gap-2">
              {proposal.reviews.map((review) => (
                <li
                  key={`${review.reviewer}-${String(review.at)}`}
                  className="text-13 text-text-2"
                >
                  <ActorText segments={reviewText(review)} />
                </li>
              ))}
            </ul>
          ) : (
            <SideNote>No reviews yet.</SideNote>
          )}
        </>
      ) : null}
      {integration ? (
        <SideNote>
          Integrated as{" "}
          <Id href={revisionPath(slug, integration.revision)}>
            {integration.revision}
          </Id>{" "}
          <CommitId commit={integration.commit} />{" "}
          <ActorText segments={integrationText(integration)} />
        </SideNote>
      ) : null}
    </SideGroup>
  );
}

function SendBackForm({
  slug,
  task,
  onDone,
}: {
  slug: string;
  task: string;
  onDone: () => void;
}) {
  const write = useTaskWrite(slug, task, (values: SendBackValues) =>
    sendBackTask(slug, task, values.note),
  );
  const form = useForm<SendBackValues>({
    resolver: zodResolver(sendBackSchema),
    defaultValues: { note: "" },
  });
  return (
    <form
      aria-label="Send the task back"
      className="flex flex-col gap-8"
      onSubmit={submit(
        form.handleSubmit((values) => {
          write.mutate(values, { onSuccess: onDone });
        }),
      )}
    >
      <Textarea
        label="Note to the task worker"
        hint="fold resumes the task worker with this note."
        rows={3}
        disabled={write.isPending}
        error={form.formState.errors.note?.message}
        {...form.register("note")}
      />
      <WriteError error={write.error} />
      <div className="flex gap-8">
        <Button
          type="submit"
          variant="primary"
          size="sm"
          pending={write.isPending}
          pendingLabel="Sending back…"
        >
          Send back
        </Button>
        <Button
          variant="ghost"
          size="sm"
          disabled={write.isPending}
          onClick={onDone}
        >
          Cancel
        </Button>
      </div>
    </form>
  );
}

function HumanReviewForm({
  slug,
  task,
  onDone,
}: {
  slug: string;
  task: string;
  onDone: () => void;
}) {
  const session = useQuery(sessionQuery());
  const write = useTaskWrite(slug, task, (mark: HumanReviewForm) =>
    setHumanReview(slug, task, mark),
  );
  const form = useForm<HumanReviewInput, unknown, HumanReviewForm>({
    resolver: zodResolver(humanReviewSchema),
    defaultValues: { reviewer: session.data?.human ?? "", reason: "" },
  });
  const me = session.data?.human;
  // "Require my review" names the signed-in human, whose name can arrive after the form opens.
  useEffect(() => {
    if (typeof me === "string" && !form.getFieldState("reviewer").isDirty) {
      form.setValue("reviewer", me);
    }
  }, [me, form]);
  return (
    <form
      aria-label="Require a human review"
      className="flex flex-col gap-8"
      onSubmit={submit(
        form.handleSubmit((values) => {
          write.mutate(values, { onSuccess: onDone });
        }),
      )}
    >
      <TextField
        label="Reviewer"
        hint="Leave empty to let any human approve."
        disabled={write.isPending}
        error={form.formState.errors.reviewer?.message}
        {...form.register("reviewer")}
      />
      <TextField
        label="Reason"
        disabled={write.isPending}
        error={form.formState.errors.reason?.message}
        {...form.register("reason")}
      />
      <WriteError error={write.error} />
      <div className="flex gap-8">
        <Button
          type="submit"
          variant="primary"
          size="sm"
          pending={write.isPending}
          pendingLabel="Requiring review…"
        >
          Require review
        </Button>
        <Button
          variant="ghost"
          size="sm"
          disabled={write.isPending}
          onClick={onDone}
        >
          Cancel
        </Button>
      </div>
    </form>
  );
}

function BlockerForm({
  slug,
  task,
  reason,
  onDone,
}: {
  slug: string;
  task: string;
  reason: string;
  onDone: () => void;
}) {
  const write = useTaskWrite(slug, task, (values: BlockerValues) =>
    setBlocker(slug, task, values.reason),
  );
  const form = useForm<BlockerValues>({
    resolver: zodResolver(blockerSchema),
    defaultValues: { reason },
  });
  return (
    <form
      aria-label="Set a blocker"
      className="flex flex-col gap-8"
      onSubmit={submit(
        form.handleSubmit((values) => {
          write.mutate(values, { onSuccess: onDone });
        }),
      )}
    >
      <Textarea
        label="Reason"
        hint="Agents cannot claim the task or start a task worker for it until a human clears the blocker."
        rows={3}
        disabled={write.isPending}
        error={form.formState.errors.reason?.message}
        {...form.register("reason")}
      />
      <WriteError error={write.error} />
      <div className="flex gap-8">
        <Button
          type="submit"
          variant="primary"
          size="sm"
          pending={write.isPending}
          pendingLabel="Setting blocker…"
        >
          Set blocker
        </Button>
        <Button
          variant="ghost"
          size="sm"
          disabled={write.isPending}
          onClick={onDone}
        >
          Cancel
        </Button>
      </div>
    </form>
  );
}

function AddDependencyForm({
  slug,
  task,
  onDone,
}: {
  slug: string;
  task: string;
  onDone: () => void;
}) {
  const write = useTaskWrite(slug, task, (needs: string) =>
    addDependency(slug, task, needs),
  );
  const form = useForm<DependencyForm>({
    resolver: zodResolver(dependencySchema(task)),
    defaultValues: { task: "" },
  });
  return (
    <form
      aria-label="Add a dependency"
      className="flex flex-col gap-8"
      onSubmit={submit(
        form.handleSubmit((values) => {
          write.mutate(values.task, { onSuccess: onDone });
        }),
      )}
    >
      <TextField
        label={`${task} needs`}
        placeholder="T-12"
        autoFocus
        disabled={write.isPending}
        error={form.formState.errors.task?.message}
        {...form.register("task")}
      />
      <WriteError error={write.error} />
      <div className="flex gap-8">
        <Button
          type="submit"
          variant="primary"
          size="sm"
          pending={write.isPending}
          pendingLabel="Adding dependency…"
        >
          Add dependency
        </Button>
        <Button
          variant="ghost"
          size="sm"
          disabled={write.isPending}
          onClick={onDone}
        >
          Cancel
        </Button>
      </div>
    </form>
  );
}

/** Takes the place of the workspace, checkpoints, changesets and proposal for a task without a proposal: the task worker's newest report, how the lead check answered it, and the human's complete and send-back controls. */
function LeadCheck({
  slug,
  task,
  now,
}: {
  slug: string;
  task: TaskDetail;
  now: number;
}) {
  const [sending, setSending] = useState(false);
  const complete = useTaskWrite(slug, task.task, () =>
    completeTask(slug, task.task),
  );
  const check = leadCheck(task);
  if (check === null) {
    return null;
  }
  const { report } = check;
  const resolved = report === null ? null : resolvedText(report, now);
  return (
    <SideGroup title="Lead check">
      <p
        className={cn(
          "flex items-center gap-6 font-medium",
          check.state === "waiting" ? "text-warn" : "text-text",
        )}
      >
        <ClipboardCheck aria-hidden="true" size={14} />
        {check.title}
      </p>
      {report === null ? (
        <SideNote>{KIND_NOTE}</SideNote>
      ) : (
        <>
          <SideNote>
            <ActorText segments={reportedText(report, now)} />
          </SideNote>
          <p className={cn("leading-normal", textFlow("prose"))}>
            {report.note}
          </p>
          {resolved ? (
            <SideNote>
              <ActorText segments={resolved} />
            </SideNote>
          ) : null}
          {report.send_back_note === null ? null : (
            <p className={cn("leading-normal", textFlow("prose"))}>
              {report.send_back_note}
            </p>
          )}
        </>
      )}
      {check.canCheck && sending ? (
        <SendBackForm
          slug={slug}
          task={task.task}
          onDone={() => {
            setSending(false);
          }}
        />
      ) : null}
      {check.canCheck && !sending ? (
        <div className="flex flex-wrap gap-8">
          <Button
            variant="primary"
            size="sm"
            pending={complete.isPending}
            pendingLabel="Completing task…"
            onClick={() => {
              complete.mutate(undefined);
            }}
          >
            Complete task
          </Button>
          <Button
            variant="ghost"
            size="sm"
            disabled={complete.isPending}
            onClick={() => {
              setSending(true);
            }}
          >
            Send back
          </Button>
        </div>
      ) : null}
      <WriteError error={complete.error} />
    </SideGroup>
  );
}

function HumanReview({ slug, task }: { slug: string; task: TaskDetail }) {
  const [adding, setAdding] = useState(false);
  const clear = useTaskWrite(slug, task.task, () =>
    clearHumanReview(slug, task.task),
  );
  const mark = task.human_review;
  const editable = task.can_change_human_review;
  const { areas } = useAreas(slug);
  const note = mainReviewNote(
    inMainArea(task.labels, areas ?? []),
    mark !== null,
  );
  if (mark === null) {
    return (
      <SideGroup
        title="Human review"
        action={
          editable && !adding ? (
            <SmallAction
              icon={UserCheck}
              onClick={() => {
                setAdding(true);
              }}
            >
              Require my review
            </SmallAction>
          ) : undefined
        }
      >
        {adding ? (
          <HumanReviewForm
            slug={slug}
            task={task.task}
            onDone={() => {
              setAdding(false);
            }}
          />
        ) : (
          <span className="text-text-2">Not required</span>
        )}
        {note === null ? null : <SideNote>{note}</SideNote>}
      </SideGroup>
    );
  }
  const lines = humanReviewLines(mark, task.human_review_set_at);
  return (
    <SideGroup title="Human review">
      <Chip tone="warn" className="self-start">
        <UserCheck aria-hidden="true" size={14} />
        Required ·
        {lines.reviewer === null ? (
          " any human"
        ) : (
          <PersonName name={lines.reviewer} />
        )}
      </Chip>
      <SideNote className="flex flex-wrap items-center gap-x-6">
        Set by <PersonName name={lines.setBy} />
        {lines.at === null ? null : ` · ${lines.at}`}
      </SideNote>
      <p className="leading-normal">{mark.reason}</p>
      {note === null ? null : <SideNote>{note}</SideNote>}
      {editable ? (
        <SmallAction
          icon={X}
          className="self-start"
          pending={clear.isPending}
          pendingLabel="Turning off human review…"
          onClick={() => {
            clear.mutate(undefined);
          }}
        >
          Turn off human review
        </SmallAction>
      ) : null}
      <WriteError error={clear.error} />
    </SideGroup>
  );
}

/** The blocker while it is set or being set: while it is set, no agent claims the task and no lead starts a task worker for it. */
function Blocker({
  slug,
  task,
  now,
  editing,
  onEditing,
}: {
  slug: string;
  task: TaskDetail;
  now: number;
  editing: boolean;
  onEditing: (editing: boolean) => void;
}) {
  const clear = useTaskWrite(slug, task.task, () =>
    clearBlocker(slug, task.task),
  );
  const view = blockerView(task, now);
  const controls = blockerControls(task);
  if (editing && controls.canSet) {
    return (
      <SideGroup title="Blocker">
        <BlockerForm
          slug={slug}
          task={task.task}
          reason={view?.reason ?? ""}
          onDone={() => {
            onEditing(false);
          }}
        />
      </SideGroup>
    );
  }
  if (view === null) {
    return null;
  }
  return (
    <SideGroup title="Blocker">
      <p className="flex items-center gap-6 font-semibold text-warn">
        <Hand aria-hidden="true" size={14} />
        No agent takes this task
      </p>
      <p className={cn("leading-normal", textFlow("prose"))}>{view.reason}</p>
      <SideNote>
        <ActorText segments={view.setBy} />
      </SideNote>
      {controls.canClear || controls.canSet ? (
        <div className="flex flex-wrap gap-x-16 gap-y-4">
          {controls.canClear ? (
            <SmallAction
              icon={X}
              pending={clear.isPending}
              pendingLabel="Clearing blocker…"
              onClick={() => {
                clear.mutate(undefined);
              }}
            >
              Clear blocker
            </SmallAction>
          ) : null}
          {controls.canSet ? (
            <SmallAction
              icon={Pencil}
              disabled={clear.isPending}
              onClick={() => {
                onEditing(true);
              }}
            >
              Change reason
            </SmallAction>
          ) : null}
        </div>
      ) : null}
      <WriteError error={clear.error} />
    </SideGroup>
  );
}

function EdgeRow({
  slug,
  row,
  onRemove,
  removing,
}: {
  slug: string;
  row: DependencyRow;
  onRemove: (() => void) | null;
  removing: boolean;
}) {
  const filters = useRouterState({ select: (s) => s.location.searchStr });
  return (
    <li
      data-edge={row.task}
      data-unfinished={row.unfinished ? "" : undefined}
      className="flex items-start gap-6"
    >
      <HrefLink
        href={taskPath(slug, row.task, filters)}
        className={cn(
          "flex min-w-0 flex-1 flex-col rounded-s hover:underline",
          focusRing,
        )}
      >
        <span className={cn("wrap-anywhere", !row.unfinished && "text-text-2")}>
          <Id className="text-text-2">{row.task}</Id> {row.title}
        </span>
        <span className="text-13 text-text-2">
          {statusLook(row.status).text}
        </span>
      </HrefLink>
      {onRemove ? (
        <IconButton
          size="sm"
          variant="ghost"
          aria-label={`Remove the dependency on ${row.task}`}
          pending={removing}
          pendingLabel={`Removing the dependency on ${row.task}…`}
          onClick={onRemove}
        >
          <X aria-hidden="true" size={14} />
        </IconButton>
      ) : null}
    </li>
  );
}

/** The prerequisites and the dependents that exist, and the form that adds a prerequisite. */
function Dependencies({
  slug,
  task,
  adding,
  onAdding,
}: {
  slug: string;
  task: TaskDetail;
  adding: boolean;
  onAdding: (adding: boolean) => void;
}) {
  const remove = useTaskWrite(slug, task.task, (needs: string) =>
    removeDependency(slug, task.task, needs),
  );
  const groups = groupDependencies(task.dependencies);
  const editable = canEditDependencies(task.status);
  const waiting = waitingText(groups);
  return (
    <section aria-label="Dependencies" className="flex flex-col gap-14">
      {groups.needs.length > 0 ? (
        <SideGroup title="Needs">
          {waiting === null ? null : <SideNote>{waiting}</SideNote>}
          <ul aria-label="Needs" className="flex flex-col gap-8">
            {groups.needs.map((row) => (
              <EdgeRow
                key={row.task}
                slug={slug}
                row={row}
                removing={remove.isPending && remove.variables === row.task}
                onRemove={
                  editable && row.removable
                    ? () => {
                        remove.mutate(row.task);
                      }
                    : null
                }
              />
            ))}
          </ul>
          <WriteError error={remove.error} />
        </SideGroup>
      ) : null}
      {groups.neededBy.length > 0 ? (
        <SideGroup title="Needed by">
          <ul aria-label="Needed by" className="flex flex-col gap-8">
            {groups.neededBy.map((row) => (
              <EdgeRow
                key={row.task}
                slug={slug}
                row={row}
                removing={false}
                onRemove={null}
              />
            ))}
          </ul>
        </SideGroup>
      ) : null}
      {editable && adding ? (
        <AddDependencyForm
          slug={slug}
          task={task.task}
          onDone={() => {
            onAdding(false);
          }}
        />
      ) : null}
    </section>
  );
}

function Links({
  slug,
  task,
  now,
}: {
  slug: string;
  task: TaskDetail;
  now: number;
}) {
  const [blocking, setBlocking] = useState(false);
  const [adding, setAdding] = useState(false);
  const controls = blockerControls(task);
  const canSet = controls.canSet && task.blocker === null && !blocking;
  const canAdd = canEditDependencies(task.status) && !adding;
  return (
    <SideCard title="Links">
      <Blocker
        slug={slug}
        task={task}
        now={now}
        editing={blocking}
        onEditing={setBlocking}
      />
      <Dependencies
        slug={slug}
        task={task}
        adding={adding}
        onAdding={setAdding}
      />
      <NothingYet text={nothingText(linkGaps(task))} />
      {canSet || canAdd ? (
        <div className="flex flex-wrap gap-x-16 gap-y-4">
          {canSet ? (
            <SmallAction
              icon={Hand}
              onClick={() => {
                setBlocking(true);
              }}
            >
              Set blocker
            </SmallAction>
          ) : null}
          {canAdd ? (
            <SmallAction
              icon={Plus}
              onClick={() => {
                setAdding(true);
              }}
            >
              Add dependency
            </SmallAction>
          ) : null}
        </div>
      ) : null}
    </SideCard>
  );
}

/** The task's events, closed until the human opens them. */
function History({ task }: { task: TaskDetail }) {
  const [shown, setShown] = useState(false);
  const [open, setOpen] = useState(false);
  const list = collapse(task.history, 2, open);
  return (
    <section aria-label="History" className="flex flex-col gap-8">
      <button
        type="button"
        aria-expanded={shown}
        onClick={() => {
          setShown(!shown);
        }}
        className={cn(
          "flex w-full items-center justify-between rounded-m px-16 py-10 font-title text-15 font-semibold text-text hover:bg-fill",
          focusRing,
        )}
      >
        History
        {shown ? (
          <ChevronUp aria-hidden="true" size={16} />
        ) : (
          <ChevronDown aria-hidden="true" size={16} />
        )}
      </button>
      {shown ? (
        <div className="flex flex-col gap-8 px-16 pb-8">
          <ul className="flex flex-col gap-8">
            {list.shown.map((event) => (
              <li
                key={event.seq}
                className="text-13 leading-normal text-text-2"
              >
                <ActorText segments={historyText(event)} />
              </li>
            ))}
          </ul>
          <MoreToggle
            text={list.more}
            open={open}
            onToggle={() => {
              setOpen(!open);
            }}
          />
        </div>
      ) : null}
    </section>
  );
}

/** The side panel: People, Work and Links on the panel's ground, each empty item folded into its group's one line, then History. */
export function TaskSide({ slug, task, live, now }: Props) {
  const withProposal = needsProposal(task);
  return (
    <aside
      aria-label="Task details"
      tabIndex={0}
      className={cn(
        "flex w-full shrink-0 flex-col gap-12 bg-bg p-14 md:w-task-side md:overflow-y-auto",
        insetFocusRing,
      )}
    >
      <SideCard title="People">
        <LabelsSide slug={slug} task={task} />
        <Responsible task={task} now={now} />
        <HumanReview slug={slug} task={task} />
        <NothingYet text={nothingText(peopleGaps(task))} />
      </SideCard>
      <SideCard title="Work">
        {withProposal ? (
          <>
            <Workspace task={task} live={live} />
            <Checkpoints slug={slug} task={task} live={live} now={now} />
            <Changesets task={task} now={now} />
            <Proposal slug={slug} task={task} />
          </>
        ) : (
          <LeadCheck slug={slug} task={task} now={now} />
        )}
        <TaskCostSection slug={slug} task={task.task} />
        {withProposal ? (
          <NothingYet text={nothingText(workGaps(task, live))} />
        ) : null}
      </SideCard>
      <Links slug={slug} task={task} now={now} />
      {/* An empty History draws nothing, so the row is left out for a task read without events. */}
      {task.history.length > 0 ? <History task={task} /> : null}
    </aside>
  );
}
