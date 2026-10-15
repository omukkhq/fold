import { zodResolver } from "@hookform/resolvers/zod";
import { useQuery } from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import {
  Bookmark,
  ChevronDown,
  ChevronUp,
  GitCommitHorizontal,
  GitPullRequestArrow,
  Plus,
  UserCheck,
  X,
} from "lucide-react";
import { useEffect, useState } from "react";
import { useForm } from "react-hook-form";
import { sessionQuery } from "@/api/queries";
import {
  addDependency,
  clearHumanReview,
  removeDependency,
  setHumanReview,
  type TaskDetail,
} from "@/api/tasks";
import { HrefLink } from "@/app/HrefLink";
import { Avatar } from "@/components/avatar/Avatar";
import { Button, IconButton } from "@/components/button/Button";
import { TextField } from "@/components/form/controls";
import { Mono } from "@/components/mono/Mono";
import { taskPath } from "@/features/board/card";
import { formatAge } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import {
  canEditDependencies,
  groupDependencies,
  needsTitle,
  type DependencyRow,
} from "./dependencies";
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
  dependencySchema,
  humanReviewSchema,
  type DependencyForm,
  type HumanReviewForm,
  type HumanReviewInput,
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
  const implementation = task.claims.find((c) => c.kind === "implementation");
  const review = task.claims.find((c) => c.kind === "review");
  return (
    <SideGroup title="Responsible">
      {implementation ? (
        <>
          <span className="inline-flex items-center gap-6">
            <Avatar name={implementation.holder} />
            <span className="font-mono text-bar text-text">
              {implementation.holder}
            </span>
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
          Review claim: <Mono>{review.holder}</Mono> · {claimNote(review, now)}
        </p>
      ) : null}
    </SideGroup>
  );
}

function Workspace({ task }: { task: TaskDetail }) {
  const ws = task.workspace;
  return (
    <SideGroup title="Workspace">
      {ws ? (
        <>
          <KeyValue name="Id" value={ws.id} />
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
      ) : (
        <Note>No workspace yet.</Note>
      )}
    </SideGroup>
  );
}

function Checkpoints({ task, now }: Props) {
  const [open, setOpen] = useState(false);
  const list = collapse(task.checkpoints, 1, open);
  return (
    <SideGroup title="Checkpoints">
      {task.checkpoints.length === 0 ? <Note>No checkpoints yet.</Note> : null}
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
  return (
    <SideGroup title="Changesets">
      {task.changesets.length === 0 ? <Note>No changesets yet.</Note> : null}
      <ul className="flex flex-col gap-8">
        {task.changesets.map((cs) => (
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
              {proposalStateText(proposal.state, proposal.version)}
            </span>
          </p>
          {proposal.reviews.length > 0 ? (
            <ul aria-label="Reviews" className="flex flex-col gap-4">
              {proposal.reviews.map((review) => (
                <li
                  key={`${review.reviewer}-${String(review.at)}`}
                  className="text-meta text-text-muted"
                >
                  {reviewText(review)}
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
          {integrationText(integration)}
        </p>
      ) : null}
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
      </SideGroup>
    );
  }
  const lines = humanReviewLines(mark);
  return (
    <SideGroup title="Human review">
      <p className="flex items-center gap-6 text-bar font-medium text-secondary-text">
        <UserCheck aria-hidden="true" size={13} />
        {lines.title}
      </p>
      <p className="text-meta text-text-muted">{lines.setBy}</p>
      <p className="text-bar leading-normal text-text">{mark.reason}</p>
      {editable ? (
        <Button
          variant="ghost"
          size="sm"
          className="self-start"
          pending={clear.isPending}
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
            key={`${event.type}-${String(event.at)}`}
            className="text-meta leading-normal text-text-muted"
          >
            {historyText(event)}
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

/** W03's side column, top to bottom as the frame orders it. */
export function TaskSide({ slug, task, now }: Props) {
  return (
    <aside className="flex w-full shrink-0 flex-col gap-20 border-border-subtle md:w-side-panel md:border-l md:pt-2 md:pl-20">
      <Responsible task={task} now={now} />
      <Workspace task={task} />
      <Checkpoints slug={slug} task={task} now={now} />
      <Changesets slug={slug} task={task} now={now} />
      <Proposal slug={slug} task={task} />
      <HumanReview slug={slug} task={task} />
      <Dependencies slug={slug} task={task} />
      <History task={task} />
    </aside>
  );
}
