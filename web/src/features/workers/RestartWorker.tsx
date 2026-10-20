import * as RadixDialog from "@radix-ui/react-dialog";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { RotateCcw, X } from "lucide-react";
import { useId, useState } from "react";
import { keys } from "@/api/keys";
import { restartWorker } from "@/api/workers";
import { Button, IconButton } from "@/components/button/Button";
import { MentionTextarea } from "@/mentions/MentionTextarea";
import { ErrorNote } from "./parts";
import { restartBody, restartFailure } from "./restart";

const FIELD_LABEL = "text-bar leading-snug font-semibold text-text";

/** The button that starts one worker again in a new Claude Code session, and the dialog that asks first. The confirm stays pending until the daemon answers. */
export function RestartWorker({
  slug,
  name,
  previousSessions,
}: {
  slug: string;
  name: string;
  /** The ids of the worker's earlier sessions, oldest first. */
  previousSessions: readonly string[];
}) {
  const queryClient = useQueryClient();
  const ids = useId();
  const [open, setOpen] = useState(false);
  const [instructions, setInstructions] = useState("");
  const restart = useMutation({
    mutationFn: () => restartWorker(slug, name, restartBody(instructions)),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: keys.workers(slug) });
      setInstructions("");
      setOpen(false);
    },
  });
  const close = () => {
    if (restart.isPending) return;
    restart.reset();
    setOpen(false);
  };
  return (
    <RadixDialog.Root
      open={open}
      onOpenChange={(next) => {
        if (next) setOpen(true);
        else close();
      }}
    >
      <RadixDialog.Trigger asChild>
        <Button
          size="sm"
          icon={<RotateCcw aria-hidden="true" size={13} />}
          aria-label={`Start ${name} in a new session`}
        >
          New session
        </Button>
      </RadixDialog.Trigger>
      <RadixDialog.Portal>
        <RadixDialog.Overlay className="fixed inset-0 bg-scrim" />
        <RadixDialog.Content className="fixed top-1/2 left-1/2 flex max-h-full w-request-dialog max-w-full -translate-x-1/2 -translate-y-1/2 flex-col overflow-y-auto rounded-l bg-raised shadow-dialog inset-ring inset-ring-border outline-none">
          <div className="flex items-center justify-between border-b border-border-subtle py-12 pr-16 pl-24">
            <RadixDialog.Title className="text-section leading-snug font-semibold text-text">
              Start {name} in a new session
            </RadixDialog.Title>
            <RadixDialog.Close asChild>
              <IconButton
                aria-label="Close"
                size="sm"
                disabled={restart.isPending}
              >
                <X aria-hidden="true" size={13} />
              </IconButton>
            </RadixDialog.Close>
          </div>
          <form
            noValidate
            onSubmit={(event) => {
              event.preventDefault();
              restart.mutate();
            }}
            className="flex flex-col"
          >
            <div className="flex flex-col gap-16 px-24 pt-18 pb-20">
              <RadixDialog.Description className="text-bar text-text-muted">
                fold stops the worker when it runs, then starts it with a new
                Claude Code session and its role's first turn. It forgets its
                current session but keeps its name, its claims and the workers
                it started.
              </RadixDialog.Description>
              {previousSessions.length > 0 ? (
                <p className="text-meta text-text-faint">
                  Earlier sessions:{" "}
                  <span className="font-mono">
                    {previousSessions.join(", ")}
                  </span>
                </p>
              ) : null}
              <div className="flex flex-col gap-6">
                <label htmlFor={`${ids}-instructions`} className={FIELD_LABEL}>
                  Instructions
                </label>
                <MentionTextarea
                  id={`${ids}-instructions`}
                  slug={slug}
                  rows={3}
                  value={instructions}
                  onValueChange={setInstructions}
                  readOnly={restart.isPending}
                  className="resize-y"
                />
              </div>
              {restart.isError ? (
                <ErrorNote failure={restartFailure(restart.error)} />
              ) : null}
            </div>
            <div className="flex items-center justify-end gap-8 border-t border-border-subtle px-24 py-12">
              <Button disabled={restart.isPending} onClick={close}>
                Cancel
              </Button>
              <Button
                type="submit"
                variant="primary"
                pending={restart.isPending}
                pendingLabel="Starting…"
                icon={<RotateCcw aria-hidden="true" size={15} />}
              >
                Start new session
              </Button>
            </div>
          </form>
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}
