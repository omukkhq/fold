import {
  type UseMutationResult,
  useIsMutating,
  useMutation,
  useQueryClient,
} from "@tanstack/react-query";
import { Archive } from "lucide-react";
import { useState } from "react";
import {
  ignoreAgentChanges,
  recordAgentChanges,
  restoreAgentChanges,
} from "@/api/agentChanges";
import { keys } from "@/api/keys";
import { Button } from "@/components/button/Button";
import { Dialog } from "@/components/overlay/Dialog";
import { Mono } from "@/components/mono/Mono";
import { WriteError } from "@/features/task/parts";
import {
  RECORD_PENDING,
  dialogText,
  failureAt,
  folderWriteKey,
  nextAsking,
  putAsideLabel,
  type AgentChange,
  type Asking,
  type SettleAction,
} from "./agentChanges";

interface Choice {
  action: SettleAction;
  confirm?: string;
}

type Settle = UseMutationResult<unknown, Error, Choice>;

export interface AgentChangeChoicesState {
  settle: Settle;
  /** The open dialog; the Inbox's popover stays closed while one is open. */
  asking: Asking | null;
  /** A write of the project folder from this page is in flight. */
  writing: boolean;
  ask: (action: "restore" | "ignore", change: AgentChange) => void;
  close: () => void;
}

/** The human's choice for a worker's change of the project folder, which stays pending until Needs you is read again, and the dialog it waits on. It lives above the Inbox's popover, so the dialog outlives the popover. */
export function useAgentChangeChoices(slug: string): AgentChangeChoicesState {
  const queryClient = useQueryClient();
  const [asking, setAsking] = useState<Asking | null>(null);
  const settle = useMutation<unknown, Error, Choice>({
    mutationKey: folderWriteKey(slug),
    mutationFn: async ({ action, confirm }) => {
      const done =
        action === "record"
          ? await recordAgentChanges(slug, confirm)
          : action === "restore"
            ? await restoreAgentChanges(slug)
            : await ignoreAgentChanges(slug);
      await Promise.all(
        [keys.needsYou(slug), keys.repository(slug), keys.project(slug)].map(
          (queryKey) => queryClient.invalidateQueries({ queryKey }),
        ),
      );
      return done;
    },
    onError: (error) => {
      setAsking((current) => nextAsking(current, { type: "failed", error }));
    },
    onSuccess: () => {
      setAsking((current) => nextAsking(current, { type: "done" }));
    },
  });
  const writing = useIsMutating({ mutationKey: folderWriteKey(slug) }) > 0;
  return {
    settle,
    asking,
    writing,
    ask: (action, change) => {
      settle.reset();
      setAsking((current) =>
        nextAsking(current, { type: "ask", action, change }),
      );
    },
    close: () => {
      if (settle.isPending) return;
      setAsking((current) => nextAsking(current, { type: "close" }));
      settle.reset();
    },
  };
}

function failureView(text: string | null) {
  return () => (text === null ? null : { field: null, text });
}

/** Put aside, Keep as mine and Record under the Needs you line of a worker's change. */
export function AgentChangeChoices({
  choices,
  change,
}: {
  choices: AgentChangeChoicesState;
  change: AgentChange;
}) {
  const { settle, asking, writing } = choices;
  const failed = failureAt(asking, settle.error, "line");
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap gap-6">
        <Button
          size="sm"
          variant="primary"
          icon={<Archive aria-hidden="true" size={14} />}
          disabled={writing}
          onClick={() => {
            choices.ask("restore", change);
          }}
        >
          {putAsideLabel(change.agents)}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={writing}
          onClick={() => {
            choices.ask("ignore", change);
          }}
        >
          Keep as mine
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={writing}
          pending={
            settle.isPending &&
            asking === null &&
            settle.variables.action === "record"
          }
          pendingLabel={RECORD_PENDING}
          onClick={() => {
            settle.mutate({ action: "record" });
          }}
        >
          Record
        </Button>
      </div>
      <WriteError
        error={failed === null ? null : settle.error}
        describe={failureView(failed)}
      />
    </div>
  );
}

function PathList({
  paths,
  more = 0,
}: {
  paths: readonly string[];
  more?: number;
}) {
  return (
    <ul className="flex min-w-0 flex-col gap-2">
      {paths.map((path) => (
        <li key={path} className="min-w-0 wrap-anywhere">
          <Mono className="text-text">{path}</Mono>
        </li>
      ))}
      {more > 0 ? (
        <li className="text-13 text-text-2">and {String(more)} more</li>
      ) : null}
    </ul>
  );
}

/** The dialog a choice waits on: the files that Put aside or Keep as mine changes, or the files a record empties, adds empty or took from a worker, which the human checks before the write goes on. */
export function AgentChangeDialog({
  choices,
}: {
  choices: AgentChangeChoicesState;
}) {
  const { settle, asking } = choices;
  const text = asking === null ? null : dialogText(asking);
  const failed = failureAt(asking, settle.error, "dialog");
  return (
    <Dialog
      open={asking !== null}
      onOpenChange={(open) => {
        if (!open) choices.close();
      }}
      title={text?.title ?? ""}
      description={text?.description}
      wide
      footer={
        <div className="flex flex-wrap justify-end gap-8">
          <Button
            variant="ghost"
            onClick={choices.close}
            disabled={settle.isPending}
          >
            Cancel
          </Button>
          <Button
            variant={text?.danger ? "danger" : "primary"}
            pending={settle.isPending}
            pendingLabel={text?.pending ?? ""}
            onClick={() => {
              if (asking === null) return;
              settle.mutate(
                asking.action === "record"
                  ? { action: "record", confirm: asking.confirmation.tree }
                  : { action: asking.action },
              );
            }}
          >
            {text?.confirm ?? ""}
          </Button>
        </div>
      }
    >
      {text?.notice ? (
        <p role="status" className="text-14 font-medium text-text">
          {text.notice}
        </p>
      ) : null}
      {asking === null ? null : asking.action === "record" ? (
        asking.confirmation.groups.map((group) => (
          <section
            key={group.key}
            aria-label={group.title}
            className="flex min-w-0 flex-col gap-4"
          >
            <h3 className="text-14 font-semibold text-text">{group.title}</h3>
            <PathList paths={group.paths} more={group.more} />
          </section>
        ))
      ) : (
        <PathList paths={asking.change.paths} />
      )}
      <WriteError
        error={failed === null ? null : settle.error}
        describe={failureView(failed)}
      />
    </Dialog>
  );
}
