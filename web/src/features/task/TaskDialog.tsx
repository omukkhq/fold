import * as RadixDialog from "@radix-ui/react-dialog";
import { skipToken, useQuery } from "@tanstack/react-query";
import { Check, Link, SearchX } from "lucide-react";
import { useState } from "react";
import { isApiError } from "@/api/errors";
import { keys } from "@/api/keys";
import { taskQuery, type LiveWorkspace, type TaskDetail } from "@/api/tasks";
import { Button } from "@/components/button/Button";
import { copyOutcome, type CopyState } from "@/components/mono/commitCopy";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { useLiveStatus } from "@/live/status";
import { useDataClock } from "@/live/useDataClock";
import { cn } from "@/lib/cn";
import { insetFocusRing } from "@/lib/focus";
import { ArchiveMenu } from "./ArchiveMenu";
import { ChecklistSection } from "./ChecklistSection";
import { DescriptionSection } from "./DescriptionSection";
import { DialogFrame } from "./DialogFrame";
import { HumanWork } from "./HumanWork";
import { needsProposal } from "./leadCheck";
import { liveStrip } from "./liveStrip";
import { MessagesSection } from "./MessagesSection";
import { TaskSide } from "./TaskSide";
import { TitleBlock } from "./TitleBlock";
import { WorkingChanges } from "./WorkingChanges";

export interface TaskDialogProps {
  slug: string;
  task: string;
  onClose: () => void;
}

function CopyLink() {
  const [state, setState] = useState<CopyState>("idle");
  const copied = state === "copied";
  return (
    <Button
      variant="ghost"
      icon={
        copied ? (
          <Check aria-hidden="true" size={15} />
        ) : (
          <Link aria-hidden="true" size={15} />
        )
      }
      onClick={() => {
        void copyOutcome(location.href, navigator.clipboard).then(setState);
      }}
      onBlur={() => {
        setState("idle");
      }}
    >
      {copied ? "Link copied" : state === "failed" ? "Not copied" : "Copy link"}
    </Button>
  );
}

function Loaded({
  slug,
  task,
  readAt,
}: {
  slug: string;
  task: TaskDetail;
  readAt: number;
}) {
  // Live workspace state is written only by the event stream, so this query observes the cache and never fetches.
  const { data: streamed } = useQuery<LiveWorkspace>({
    queryKey: keys.workspaceLive(slug, task.workspace ?? ""),
    queryFn: skipToken,
  });
  const status = useLiveStatus();
  const now = useDataClock([
    readAt,
    status.lastEventAt,
    streamed?.last_activity_at,
  ]);
  const live = task.workspace === null ? null : (streamed ?? null);
  return (
    <div className="flex flex-col md:min-h-0 md:flex-row">
      <div
        role="region"
        aria-label="Task"
        tabIndex={0}
        className={cn(
          "flex min-w-0 flex-1 flex-col gap-24 px-24 pt-22 pb-24 md:overflow-y-auto md:pr-28",
          insetFocusRing,
        )}
      >
        <TitleBlock
          slug={slug}
          task={task}
          strip={liveStrip(task, live, now)}
        />
        <HumanWork slug={slug} task={task} />
        <DescriptionSection slug={slug} task={task} />
        <ChecklistSection slug={slug} task={task} />
        {needsProposal(task) ? (
          <WorkingChanges
            slug={slug}
            workspace={task.workspace}
            info={task.workspace_info}
            live={live}
            now={now}
          />
        ) : null}
        <MessagesSection slug={slug} task={task} />
      </div>
      <TaskSide slug={slug} task={task} live={live} now={now} />
    </div>
  );
}

function Loading() {
  return (
    <div className="flex flex-col gap-12 px-24 pt-22 pb-24">
      <RadixDialog.Title className="sr-only">
        Loading the task
      </RadixDialog.Title>
      <Skeleton />
      <Skeleton />
      <Skeleton />
    </div>
  );
}

/**
 * The task opens over the board, which stays mounted beneath it. Every value on screen is the daemon's: a write
 * shows its pending state until the task has been read again.
 */
export function TaskDialog({ slug, task, onClose }: TaskDialogProps) {
  const query = useQuery(taskQuery(slug, task));
  let body;
  if (query.isPending) {
    body = <Loading />;
  } else if (query.isError && isApiError(query.error, "task_not_found")) {
    body = (
      <div className="px-24 pt-22 pb-24">
        <RadixDialog.Title className="sr-only">
          Task not found
        </RadixDialog.Title>
        <EmptyPanel icon={SearchX} title={`${task} does not exist`}>
          The task may have been deleted or archived. Its history stays in the
          activity list.
        </EmptyPanel>
      </div>
    );
  } else if (query.isError) {
    body = (
      <>
        <RadixDialog.Title className="sr-only">
          The task could not load
        </RadixDialog.Title>
        <ErrorBanner
          title="The task could not load."
          message="The daemon did not answer."
          onRetry={() => void query.refetch()}
        />
      </>
    );
  } else {
    body = (
      <Loaded slug={slug} task={query.data} readAt={query.dataUpdatedAt} />
    );
  }
  return (
    <DialogFrame
      crumb={task}
      crumbIsId
      actions={
        query.isError ? undefined : (
          <>
            <CopyLink />
            {query.isSuccess ? (
              <ArchiveMenu slug={slug} task={task} onArchived={onClose} />
            ) : null}
          </>
        )
      }
      described={query.isSuccess && Boolean(query.data.status_reason)}
      split={query.isSuccess}
      onClose={onClose}
    >
      {body}
    </DialogFrame>
  );
}
