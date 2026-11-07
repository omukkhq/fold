import { useMutation, useQueryClient } from "@tanstack/react-query";
import { dismissSharedChanges } from "@/api/repository";
import { keys } from "@/api/keys";
import { Button } from "@/components/button/Button";
import { WriteError } from "@/features/task/parts";
import { dismissLabel, settleFailure } from "./agentChanges";

/** Dismiss under the Needs you line of a changed shared file. It ends every such item and stays pending until Needs you is read again. */
export function SharedChangeDismiss({
  slug,
  count,
}: {
  slug: string;
  count: number;
}) {
  const queryClient = useQueryClient();
  const dismiss = useMutation({
    mutationFn: async () => {
      const done = await dismissSharedChanges(slug);
      await queryClient.invalidateQueries({ queryKey: keys.needsYou(slug) });
      return done;
    },
  });
  const failed = dismiss.error ? settleFailure(dismiss.error) : null;
  return (
    <div className="flex flex-col gap-4">
      <div>
        <Button
          size="sm"
          variant="ghost"
          pending={dismiss.isPending}
          pendingLabel="Dismissing…"
          onClick={() => {
            dismiss.mutate();
          }}
        >
          {dismissLabel(count)}
        </Button>
      </div>
      <WriteError
        error={failed === null ? null : dismiss.error}
        describe={() =>
          failed === null ? null : { field: null, text: failed }
        }
      />
    </div>
  );
}
