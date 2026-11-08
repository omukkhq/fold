import { useMutation, useQueryClient } from "@tanstack/react-query";
import { RotateCcw } from "lucide-react";
import { keys } from "@/api/keys";
import { restartWorker } from "@/api/workers";
import { Button } from "@/components/button/Button";
import { ErrorNote } from "./parts";
import { restartFailure } from "./restart";

/**
 * Restart worker for a stalled worker, the same as `fld worker restart <name>`: one press starts it again in a new
 * Claude Code session, and the button stays pending until the daemon answers. It asks nothing first, because it sits in
 * panels that close when the pointer leaves them; the stall it ends is named next to it.
 */
export function RestartStalled({ slug, name }: { slug: string; name: string }) {
  const queryClient = useQueryClient();
  const restart = useMutation({
    mutationFn: () => restartWorker(slug, name, {}),
    onSuccess: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: keys.workers(slug) }),
        queryClient.invalidateQueries({ queryKey: keys.needsYou(slug) }),
      ]),
  });
  return (
    <div className="flex flex-col gap-8">
      <Button
        size="sm"
        className="self-start"
        pending={restart.isPending}
        pendingLabel="Restarting…"
        icon={<RotateCcw aria-hidden="true" size={14} />}
        aria-label={`Restart ${name} in a new session`}
        onClick={() => {
          restart.mutate();
        }}
      >
        Restart worker
      </Button>
      {restart.isError ? (
        <ErrorNote failure={restartFailure(restart.error)} />
      ) : null}
    </div>
  );
}
