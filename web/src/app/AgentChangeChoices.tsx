import {
  type UseMutationResult,
  useMutation,
  useQueryClient,
} from "@tanstack/react-query";
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
import {
  ACTION_LABELS,
  PENDING_LABELS,
  confirmationOf,
  settleFailure,
  type RecordConfirmation,
  type SettleAction,
} from "./agentChanges";

interface Choice {
  action: SettleAction;
  confirm?: string;
}

type Settle = UseMutationResult<unknown, Error, Choice>;

/** The human's choice for a worker's change of the project folder, which stays pending until Needs you is read again, and the confirmation a record waits for. It lives above the Inbox's popover, so the dialog outlives the popover. */
export function useAgentChangeChoices(slug: string): {
  settle: Settle;
  confirmation: RecordConfirmation | null;
  dismiss: () => void;
} {
  const queryClient = useQueryClient();
  const [confirmation, setConfirmation] = useState<RecordConfirmation | null>(
    null,
  );
  const settle = useMutation<unknown, Error, Choice>({
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
      setConfirmation(confirmationOf(error));
    },
    onSuccess: () => {
      setConfirmation(null);
    },
  });
  return {
    settle,
    confirmation,
    dismiss: () => {
      setConfirmation(null);
      settle.reset();
    },
  };
}

const ACTIONS: readonly SettleAction[] = ["record", "restore", "ignore"];

/** Record, Restore and Ignore under the Needs you line of a worker's change. */
export function AgentChangeChoices({ settle }: { settle: Settle }) {
  const failed = settle.error ? settleFailure(settle.error) : null;
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap gap-6">
        {ACTIONS.map((action) => (
          <Button
            key={action}
            size="sm"
            variant={action === "record" ? "primary" : "quiet"}
            disabled={settle.isPending}
            pending={settle.isPending && settle.variables.action === action}
            pendingLabel={PENDING_LABELS[action]}
            onClick={() => {
              settle.mutate({ action });
            }}
          >
            {ACTION_LABELS[action]}
          </Button>
        ))}
      </div>
      {failed === null ? null : (
        <span role="alert" className="text-13 font-medium text-bad">
          {failed}
        </span>
      )}
    </div>
  );
}

/** The files a record empties, adds empty or took from a worker, which the human checks before the record goes on. */
export function RecordConfirmDialog({
  confirmation,
  settle,
  onClose,
}: {
  confirmation: RecordConfirmation | null;
  settle: Settle;
  onClose: () => void;
}) {
  const failed =
    settle.error && confirmationOf(settle.error) === null
      ? settleFailure(settle.error)
      : null;
  return (
    <Dialog
      open={confirmation !== null}
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      title="Record these files?"
      description="Check them first: fold found files that are often a mistake."
      wide
      footer={
        <div className="flex flex-wrap items-center justify-end gap-8">
          {failed === null ? null : (
            <span role="alert" className="mr-auto text-13 font-medium text-bad">
              {failed}
            </span>
          )}
          <Button variant="quiet" onClick={onClose} disabled={settle.isPending}>
            Cancel
          </Button>
          <Button
            variant="primary"
            pending={settle.isPending}
            pendingLabel={PENDING_LABELS.record}
            onClick={() => {
              if (confirmation !== null) {
                settle.mutate({ action: "record", confirm: confirmation.tree });
              }
            }}
          >
            Record anyway
          </Button>
        </div>
      }
    >
      {confirmation?.groups.map((group) => (
        <section
          key={group.key}
          aria-label={group.title}
          className="flex flex-col gap-4"
        >
          <h3 className="text-14 font-semibold text-text">{group.title}</h3>
          <ul className="flex flex-col gap-2">
            {group.paths.map((path) => (
              <li key={path}>
                <Mono className="text-text-2">{path}</Mono>
              </li>
            ))}
            {group.more > 0 ? (
              <li className="text-13 text-text-2">
                and {String(group.more)} more
              </li>
            ) : null}
          </ul>
        </section>
      ))}
    </Dialog>
  );
}
