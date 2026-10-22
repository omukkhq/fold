import { zodResolver } from "@hookform/resolvers/zod";
import { useQuery } from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import {
  Bookmark,
  ChevronDown,
  ClipboardCheck,
  ChevronUp,
  GitCommitHorizontal,
  GitPullRequestArrow,
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
import { Button, IconButton } from "@/components/button/Button";
import { Textarea, TextField } from "@/components/form/controls";
import { CommitId } from "@/components/mono/CommitId";
import { Mono } from "@/components/mono/Mono";
import { textFlow } from "@/components/text/textFlow";
import { taskPath } from "@/features/board/card";
import { TaskCostSection } from "@/features/cost/TaskCostSection";
import { formatAge } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { ActorText } from "@/people/ActorText";
import { PersonName } from "@/people/PersonName";
import {
  canEditDependencies,
  groupDependencies,
  needsTitle,
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
  KeyValue,
  SideGroup,
  SmallAction,
  StatusMark,
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
import {
  checkpointText,
  claimNote,
  collapse,
  historyText,
  humanReviewLines,
  integrationText,
  proposalStateText,
  reviewText,
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
      className="self-start px-0 py-2"
      onClick={onToggle}
    >
      {text}
    </SmallAction>
  );
}

function Note({ children }: { children: string }) {
  return <p className="text-bar leading-normal text-text-muted">{children}</p>;
}

function Responsible({ task, now }: { task: TaskDetail; now: number }) {
  // Every claim of a task read is its implementation claim.
  const implementation = task.claims[0];
  // The task read lists implementation claims only, so the review row draws for no task.
  const review = task.claims.find((c) => (c.kind as string) === "review");
  return (
    <SideGroup title="Responsible">
      {implementation ? (
        <>
          <span className="inline-flex items-center gap-6">
            <PersonName name={implementation.holder} className="text-body" />
          </span>
          <p
            className={cn(
              "text-meta leading-normal",
              implementation.stale ? "text-secondary-text" : "text-text-muted",
            )}
          >
            {claimNote(implementation, now)}
          </p>
        </>
      ) : (
        <Note>Nobody holds the implementation claim.</Note>
      )}
      {review ? (
        <p className="text-meta leading-normal text-text-muted">
          Review claim: <PersonName name={review.holder} className="text-bar" />{" "}
          · {claimNote(review, now)}
        </p>
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
    return (
      <SideGroup title="Workspace">
        <Note>No workspace yet.</Note>
      </SideGroup>
    );
  }
  return (
    <SideGroup title="Workspace">
      <KeyValue name="Id" value={task.workspace} />
      {ws ? (
        <>
          <KeyValue name="Directory" value={ws.directory} />
          <KeyValue
            name="Base"
            value={
              ws.base === ws.main
                ? `${ws.base} (main)`
                : `${ws.base} (main is ${ws.main})`
            }
          />
          {ws.behind > 0 ? (
            <p className="flex items-center gap-6 self-start rounded-s bg-sunken px-8 py-5 text-meta text-text-muted">
              <GitPullRequestArrow aria-hidden="true" size={13} />
              Main moved {ws.behind}{" "}
              {ws.behind === 1 ? "revision" : "revisions"} since this base
            </p>
          ) : null}
        </>
      ) : live ? (
        <KeyValue name="Base" value={live.base} />
      ) : null}
    </SideGroup>
  );
}

function Checkpoints({ task, live, now }: Props) {
  const [open, setOpen] = useState(false);
  const all = recordedCheckpoints(task.checkpoints, live);
  const list = collapse(all, 1, open);
  return (
    <SideGroup title="Checkpoints">
      {all.length === 0 ? <Note>No checkpoints yet.</Note> : null}
      <ul className="flex flex-col gap-8">
        {list.shown.map((ck) => (
          <li key={ck.id} className="flex items-center gap-8">
            <Bookmark
              aria-hidden="true"
              size={13}
              className="shrink-0 text-text-faint"
            />
            <Mono className="text-meta text-text-muted">{ck.id}</Mono>
            <span className="min-w-0 flex-1 truncate text-bar text-text">
              {checkpointText(ck)}
            </span>
            <span className="shrink-0 text-meta text-text-faint">
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

function Changesets({ task, now }: Props) {
  const changesets = task.changesets;
  return (
    <SideGroup title="Changesets">
      {changesets.length === 0 ? <Note>No changesets yet.</Note> : null}
      <ul className="flex flex-col gap-8">
        {changesets.map((cs) => (
          <li key={cs.id} className="flex items-center gap-8">
            <GitCommitHorizontal
              aria-hidden="true"
              size={14}
              className="shrink-0 text-text-muted"
            />
            <Mono className="text-meta text-text-muted">{cs.id}</Mono>
            <span className="min-w-0 flex-1 truncate text-bar text-text">
              {cs.message}
            </span>
            <span className="shrink-0 text-meta text-text-faint">
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
  return (
    <SideGroup title="Proposal">
      {proposal ? (
        <>
          <p className="flex items-center gap-8 text-bar text-text">
            <Mono href={proposalPath(slug, proposal.id)}>{proposal.id}</Mono>
            <span className="text-text-muted">
              {proposalStateText(
                proposal.state,
                proposal.version,
                proposal.review_claim,
              )}
            </span>
          </p>
          {proposal.reviews.length > 0 ? (
            <ul aria-label="Reviews" className="flex flex-col gap-4">
              {proposal.reviews.map((review) => (
                <li
                  key={`${review.reviewer}-${String(review.at)}`}
                  className="text-meta text-text-muted"
                >
                  <ActorText
                    segments={reviewText(review)}
                    personClassName="text-bar"
                  />
                </li>
              ))}
            </ul>
          ) : (
            <Note>No reviews yet.</Note>
          )}
        </>
      ) : (
        <Note>
          None yet. It opens when the worker calls create_proposal; the card
          then moves to Review.
        </Note>
      )}
      {integration ? (
        <p className="text-meta text-text-muted">
          Integrated as{" "}
          <Mono href={revisionPath(slug, integration.revision)}>
            {integration.revision}
          </Mono>{" "}
          <CommitId commit={integration.commit} />{" "}
          <ActorText
            segments={integrationText(integration)}
            personClassName="text-bar"
          />
        </p>
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

/** Takes the proposal group's place for a task without a proposal: the task worker's newest report, how the lead check answered it, and the human's complete and send-back controls. */
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
          "flex items-center gap-6 text-bar font-medium",
          check.state === "waiting" ? "text-secondary-text" : "text-text",
        )}
      >
        <ClipboardCheck aria-hidden="true" size={13} />
        {check.title}
      </p>
      {report === null ? (
        <Note>{KIND_NOTE}</Note>
      ) : (
        <>
          <p className="text-meta text-text-muted">
            <ActorText
              segments={reportedText(report, now)}
              personClassName="text-bar"
            />
          </p>
          <p
            className={cn(
              "text-bar leading-normal text-text",
              textFlow("prose"),
            )}
          >
            {report.note}
          </p>
          {resolved ? (
            <p className="text-meta text-text-muted">
              <ActorText segments={resolved} personClassName="text-bar" />
            </p>
          ) : null}
          {report.send_back_note === null ? null : (
            <p
              className={cn(
                "text-bar leading-normal text-text",
                textFlow("prose"),
              )}
            >
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
        <div className="flex gap-8">
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
      <SideGroup title="Human review">
        {adding ? (
          <HumanReviewForm
            slug={slug}
            task={task.task}
            onDone={() => {
              setAdding(false);
            }}
          />
        ) : (
          <div className="flex items-center gap-8">
            <span className="flex-1 text-bar text-text-muted">
              Not required
            </span>
            {editable ? (
              <SmallAction
                icon={UserCheck}
                onClick={() => {
                  setAdding(true);
                }}
              >
                Require my review
              </SmallAction>
            ) : null}
          </div>
        )}
        {note === null ? null : (
          <p className="text-meta leading-normal text-text-muted">{note}</p>
        )}
      </SideGroup>
    );
  }
  const lines = humanReviewLines(mark, task.human_review_set_at);
  return (
    <SideGroup title="Human review">
      <p className="flex items-center gap-6 text-bar font-medium text-secondary-text">
        <UserCheck aria-hidden="true" size={13} />
        Required ·
        {lines.reviewer === null ? (
          "any human"
        ) : (
          <PersonName name={lines.reviewer} className="text-body" />
        )}
      </p>
      <p className="flex flex-wrap items-center gap-x-6 text-meta text-text-muted">
        Set by <PersonName name={lines.setBy} className="text-bar" />
        {lines.at === null ? null : ` · ${lines.at}`}
      </p>
      <p className="text-bar leading-normal text-text">{mark.reason}</p>
      {note === null ? null : (
        <p className="text-meta leading-normal text-text-muted">{note}</p>
      )}
      {editable ? (
        <Button
          variant="ghost"
          size="sm"
          className="self-start"
          pending={clear.isPending}
          pendingLabel="Turning off human review…"
          onClick={() => {
            clear.mutate(undefined);
          }}
        >
          Turn off human review
        </Button>
      ) : null}
      <WriteError error={clear.error} />
    </SideGroup>
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

/** The blocker: while it is set, no agent claims the task and no lead starts a task worker for it. */
function Blocker({
  slug,
  task,
  now,
}: {
  slug: string;
  task: TaskDetail;
  now: number;
}) {
  const [editing, setEditing] = useState(false);
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
            setEditing(false);
          }}
        />
      </SideGroup>
    );
  }
  if (view === null) {
    return (
      <SideGroup title="Blocker">
        <div className="flex items-center gap-8">
          <span className="flex-1 text-bar text-text-muted">None</span>
          {controls.canSet ? (
            <SmallAction
              icon={Hand}
              onClick={() => {
                setEditing(true);
              }}
            >
              Set blocker
            </SmallAction>
          ) : null}
        </div>
      </SideGroup>
    );
  }
  return (
    <SideGroup title="Blocker">
      <div className="flex flex-col gap-6 rounded-m bg-warning-soft px-12 py-10">
        <p className="flex items-center gap-6 text-bar font-semibold text-warning">
          <Hand aria-hidden="true" size={13} />
          No agent takes this task
        </p>
        <p
          className={cn("text-bar leading-normal text-text", textFlow("prose"))}
        >
          {view.reason}
        </p>
        <p className="text-meta text-text-muted">
          <ActorText segments={view.setBy} personClassName="text-bar" />
        </p>
      </div>
      {controls.canClear || controls.canSet ? (
        <div className="flex gap-4">
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
                setEditing(true);
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
      className="flex items-center gap-8"
    >
      <HrefLink
        href={taskPath(slug, row.task, filters)}
        className={cn(
          "flex min-w-0 flex-1 items-center gap-8 rounded-s hover:underline",
          focusRing,
        )}
      >
        <Mono className="text-meta text-text-muted">{row.task}</Mono>
        <span
          className={cn(
            "min-w-0 flex-1 truncate text-bar",
            row.unfinished ? "text-text" : "text-text-muted",
          )}
        >
          {row.title}
        </span>
      </HrefLink>
      <StatusMark status={row.status} />
      {onRemove ? (
        <IconButton
          size="sm"
          aria-label={`Remove the dependency on ${row.task}`}
          pending={removing}
          pendingLabel={`Removing the dependency on ${row.task}…`}
          onClick={onRemove}
        >
          <X aria-hidden="true" size={13} />
        </IconButton>
      ) : null}
    </li>
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

function Dependencies({ slug, task }: { slug: string; task: TaskDetail }) {
  const [adding, setAdding] = useState(false);
  const remove = useTaskWrite(slug, task.task, (needs: string) =>
    removeDependency(slug, task.task, needs),
  );
  const groups = groupDependencies(task.dependencies);
  const editable = canEditDependencies(task.status);
  return (
    <SideGroup title="Dependencies">
      <h4 className="text-meta font-semibold text-text-muted">
        {needsTitle(groups)}
      </h4>
      {groups.needs.length === 0 ? <Note>No prerequisites.</Note> : null}
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
      <h4 className="text-meta font-semibold text-text-muted">Needed by</h4>
      {groups.neededBy.length === 0 ? (
        <Note>No task waits for this one.</Note>
      ) : null}
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
      {editable ? (
        adding ? (
          <AddDependencyForm
            slug={slug}
            task={task.task}
            onDone={() => {
              setAdding(false);
            }}
          />
        ) : (
          <SmallAction
            icon={Plus}
            className="self-start px-0 py-4 text-bar"
            onClick={() => {
              setAdding(true);
            }}
          >
            Add dependency
          </SmallAction>
        )
      ) : null}
    </SideGroup>
  );
}

function History({ task }: { task: TaskDetail }) {
  const [open, setOpen] = useState(false);
  const list = collapse(task.history, 2, open);
  return (
    <SideGroup title="History">
      <ul className="flex flex-col gap-8">
        {list.shown.map((event) => (
          <li
            key={event.seq}
            className="text-meta leading-normal text-text-muted"
          >
            <ActorText
              segments={historyText(event)}
              personClassName="text-bar"
            />
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

/** The side column, top to bottom. */
export function TaskSide({ slug, task, live, now }: Props) {
  return (
    <aside className="flex w-full shrink-0 flex-col gap-20 border-border-subtle md:w-side-panel md:border-l md:pt-2 md:pl-20">
      <LabelsSide slug={slug} task={task} />
      <Responsible task={task} now={now} />
      {needsProposal(task) ? (
        <>
          <Workspace task={task} live={live} />
          <Checkpoints slug={slug} task={task} live={live} now={now} />
          <Changesets slug={slug} task={task} live={live} now={now} />
          <Proposal slug={slug} task={task} />
        </>
      ) : (
        <LeadCheck slug={slug} task={task} now={now} />
      )}
      <Blocker slug={slug} task={task} now={now} />
      <HumanReview slug={slug} task={task} />
      <Dependencies slug={slug} task={task} />
      <TaskCostSection slug={slug} task={task.task} />
      {/* An empty History draws nothing, so the group is left out for a task read without events. */}
      {task.history.length > 0 ? <History task={task} /> : null}
    </aside>
  );
}
