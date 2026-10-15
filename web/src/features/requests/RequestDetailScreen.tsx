import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, RefreshCw } from "lucide-react";
import { useState } from "react";
import { boardCards, boardQuery } from "@/api/board";
import { keys } from "@/api/keys";
import {
  proposalsQuery,
  requestQuery,
  requestThreadQuery,
  requestsQuery,
} from "@/api/queries";
import {
  approveRequestDraft,
  closeRequest,
  editRequestDraft,
  requestDraftChanges,
  sendRequestMessage,
  type RequestDetail,
} from "@/api/requests";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import {
  buttonClasses,
  buttonIconClass,
} from "@/components/button/buttonStyles";
import { Skeleton } from "@/components/states/Skeleton";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { useNow } from "@/live/useNow";
import { knownFrom } from "@/mentions/parse";
import {
  panelKind,
  threadClosedText,
  threadTarget,
  visibleActions,
  type RequestAction,
} from "./actions";
import { CloseDialog } from "./CloseDialog";
import {
  checkDraft,
  draftBody,
  formFrom,
  refusalText,
  type DraftForm,
} from "./draft";
import { DraftEditor } from "./DraftEditor";
import { describeFailure, reloadsRequest, type Failure } from "./errors";
import { FailurePanel } from "./FailurePanel";
import { createdTasks, shownDraft } from "./present";
import {
  BodySection,
  CommentsSection,
  Crumb,
  DraftView,
  RequestHeader,
  ThreadSection,
  VersionTabs,
} from "./RequestContent";
import {
  ClosedPanel,
  CreatedTasksPanel,
  DecisionPanel,
  DrafterBlock,
  EditingPanel,
  VersionsBlock,
  ViewerPanel,
} from "./SidePanel";

interface RequestDetailScreenProps {
  slug: string;
  id: string;
  /** The signed-in human; null before a name is given. */
  human: string | null;
}

type Write =
  | { action: "approve"; version: number }
  | { action: "request_changes"; version: number; comment: string }
  | { action: "edit"; form: DraftForm; note: string }
  | { action: "close"; reason: string };

function send(slug: string, id: string, write: Write): Promise<RequestDetail> {
  switch (write.action) {
    case "approve":
      return approveRequestDraft(slug, id, write.version);
    case "request_changes":
      return requestDraftChanges(slug, id, write.version, write.comment);
    case "edit":
      return editRequestDraft(slug, id, draftBody(write.form, write.note));
    case "close":
      return closeRequest(slug, id, write.reason);
  }
}

export function RequestDetailPending({
  slug,
  id,
}: {
  slug: string;
  id: string;
}) {
  return (
    <div className="flex flex-col gap-16 px-24 pt-18 pb-24">
      <Crumb slug={slug} id={id} />
      <Skeleton />
      <div className="flex items-start gap-24">
        <Skeleton className="flex-1" />
        <Skeleton className="w-side-panel shrink-0" />
      </div>
    </div>
  );
}

export function RequestDetailScreen({
  slug,
  id,
  human,
}: RequestDetailScreenProps) {
  const queryClient = useQueryClient();
  const query = useQuery(requestQuery(slug, id));
  const detail = query.data ?? null;
  const threadId = detail?.thread?.id ?? null;
  const thread = useQuery({
    ...requestThreadQuery(slug, id, threadId ?? ""),
    enabled: threadId !== null,
  });
  const proposals = useQuery({
    ...proposalsQuery(slug),
    enabled: detail !== null,
  });
  const requests = useQuery({
    ...requestsQuery(slug),
    enabled: detail !== null,
  });
  const board = useQuery({
    ...boardQuery(slug),
    enabled: (detail?.tasks.length ?? 0) > 0,
  });
  const now = useNow(true, 30_000);
  const [chosen, setChosen] = useState<number | null>(null);
  const [editing, setEditing] = useState<DraftForm | null>(null);
  const [note, setNote] = useState("");
  const [closing, setClosing] = useState(false);
  const [failure, setFailure] = useState<{
    action: RequestAction;
    failure: Failure;
  } | null>(null);
  const [threadFailure, setThreadFailure] = useState<Failure | null>(null);

  // The daemon answers a write with the request as it stored it; the screen shows that answer, never a guess.
  const write = useMutation({
    mutationFn: (input: Write) => send(slug, id, input),
    onMutate: () => {
      setFailure(null);
    },
    onSuccess: async (stored, input) => {
      queryClient.setQueryData(keys.request(slug, id), stored);
      if (input.action === "edit") {
        setEditing(null);
        setNote("");
      }
      setChosen(null);
      await Promise.all([
        queryClient.invalidateQueries({
          queryKey: keys.requests(slug),
          exact: true,
        }),
        input.action === "approve"
          ? queryClient.invalidateQueries({ queryKey: keys.board(slug) })
          : Promise.resolve(),
      ]);
    },
    onError: async (error, input) => {
      setFailure({ action: input.action, failure: describeFailure(error) });
      if (reloadsRequest(error)) {
        await queryClient.invalidateQueries({
          queryKey: keys.request(slug, id),
        });
      }
    },
  });
  const message = useMutation({
    mutationFn: (input: { body: string; to: string[] | undefined }) =>
      sendRequestMessage(slug, id, input.body, input.to),
    onMutate: () => {
      setThreadFailure(null);
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: keys.request(slug, id) });
    },
    onError: (error) => {
      setThreadFailure(describeFailure(error));
    },
  });

  if (query.isPending) return <RequestDetailPending slug={slug} id={id} />;
  if (query.isError || detail === null) {
    const missing = detail === null && !query.isError;
    return (
      <div className="flex flex-col gap-16 px-24 pt-18 pb-24">
        <Crumb slug={slug} id={id} />
        <FailurePanel
          title={
            missing
              ? `${id} does not exist in this project`
              : `Could not load ${id}`
          }
          text={
            missing
              ? "The id may be mistyped, or the link may come from another project. Open the list to find the request."
              : "The daemon answered with an error. Nothing is lost; try again."
          }
          code={
            missing ? "request_not_found" : describeFailure(query.error).code
          }
          action={
            missing ? (
              <HrefLink
                href={`/p/${slug}/requests`}
                className={cn(buttonClasses("secondary", "md"), focusRing)}
              >
                <span className={buttonIconClass("secondary")}>
                  <ArrowLeft aria-hidden="true" size={15} />
                </span>
                Back to requests
              </HrefLink>
            ) : (
              <Button
                icon={<RefreshCw aria-hidden="true" size={15} />}
                onClick={() => void query.refetch()}
              >
                Try again
              </Button>
            )
          }
        />
      </div>
    );
  }

  const r = detail.request;
  const known = knownFrom({
    proposals: proposals.data?.map((p) => p.id),
    requests: requests.data?.map((q) => q.id),
  });
  const latest = detail.drafts.at(-1) ?? null;
  const draft = shownDraft(detail.drafts, chosen);
  const actions = visibleActions(r, human);
  const kind = panelKind(r, human, editing !== null);
  const pending = write.isPending ? write.variables.action : null;
  const failureOf = (action: RequestAction) =>
    failure?.action === action ? failure.failure : null;
  const decisionFailure =
    failure && failure.action !== "edit" && failure.action !== "close"
      ? failure.failure
      : null;
  const target = threadTarget(r, human);

  const save = () => {
    if (editing === null) return;
    const refusal = checkDraft(editing);
    if (refusal) {
      setFailure({
        action: "edit",
        failure: { text: refusalText(refusal), code: null },
      });
      return;
    }
    write.mutate({ action: "edit", form: editing, note });
  };

  return (
    <div className="flex flex-col gap-16 px-24 pt-18 pb-24">
      <Crumb slug={slug} id={id} />
      <RequestHeader detail={detail} now={now} />
      <div className="flex flex-col items-start gap-24 lg:flex-row">
        <div className="flex w-full min-w-0 flex-1 flex-col gap-24">
          <BodySection slug={slug} detail={detail} known={known} />
          {editing !== null ? (
            <DraftEditor
              slug={slug}
              form={editing}
              next={(r.latest_version ?? 0) + 1}
              locked={pending === "edit"}
              onChange={setEditing}
            />
          ) : (
            <DraftView
              slug={slug}
              detail={detail}
              draft={draft}
              known={known}
              now={now}
              tabs={
                draft && detail.drafts.length > 1 ? (
                  <VersionTabs
                    drafts={detail.drafts}
                    shown={draft.version}
                    disabled={pending !== null}
                    onShow={setChosen}
                  />
                ) : null
              }
            />
          )}
          <CommentsSection
            slug={slug}
            detail={detail}
            known={known}
            now={now}
          />
          <ThreadSection
            slug={slug}
            detail={detail}
            messages={thread.data?.messages}
            known={known}
            now={now}
            target={target.label}
            closed={threadClosedText(r)}
            failure={threadFailure}
            onSend={async (body) => {
              await message.mutateAsync({ body, to: target.to });
            }}
          />
        </div>
        <aside
          aria-label="Request actions"
          className="flex w-full shrink-0 flex-col gap-20 border-border-subtle lg:w-side-panel lg:border-l lg:pl-20"
        >
          {kind === "approved" ? (
            <CreatedTasksPanel
              slug={slug}
              tasks={createdTasks(
                detail,
                board.data ? boardCards(board.data) : undefined,
              )}
            />
          ) : kind === "closed" ? (
            <ClosedPanel slug={slug} detail={detail} known={known} />
          ) : kind === "viewer" ? (
            <ViewerPanel detail={detail} />
          ) : kind === "editing" ? (
            <EditingPanel
              slug={slug}
              detail={detail}
              note={note}
              saving={pending === "edit"}
              failure={failureOf("edit")}
              onNote={setNote}
              onSave={save}
              onCancel={() => {
                setEditing(null);
                setNote("");
                setFailure(null);
              }}
            />
          ) : (
            <DecisionPanel
              slug={slug}
              detail={detail}
              actions={actions}
              pending={pending}
              failure={decisionFailure}
              cards={latest?.cards.length ?? 0}
              onApprove={() => {
                write.mutate({
                  action: "approve",
                  version: r.latest_version ?? 0,
                });
              }}
              onRequestChanges={async (comment) => {
                await write.mutateAsync({
                  action: "request_changes",
                  version: r.latest_version ?? 0,
                  comment,
                });
              }}
              onEdit={() => {
                if (latest) {
                  setFailure(null);
                  setEditing(formFrom(latest));
                }
              }}
              onClose={() => {
                setFailure(null);
                setClosing(true);
              }}
            />
          )}
          <DrafterBlock detail={detail} now={now} />
          <VersionsBlock detail={detail} human={human} now={now} />
        </aside>
      </div>
      <CloseDialog
        slug={slug}
        id={id}
        open={closing}
        onOpenChange={setClosing}
        failure={failureOf("close")}
        onCloseRequest={async (reason) => {
          await write.mutateAsync({ action: "close", reason });
        }}
      />
    </div>
  );
}
