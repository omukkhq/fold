import { useMutation, useQueryClient } from "@tanstack/react-query";
import { keys } from "@/api/keys";

/**
 * A human write. The mutation stays pending until the task has been read again, so the dialog never shows a value
 * the daemon has not confirmed; the board refreshes in the same step because a card shows the same fields, and so does the label list.
 */
export function useTaskWrite<V>(
  slug: string,
  task: string,
  write: (vars: V) => Promise<unknown>,
) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (vars: V) => {
      await write(vars);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: keys.task(slug, task) }),
        queryClient.invalidateQueries({ queryKey: keys.board(slug) }),
        queryClient.invalidateQueries({ queryKey: keys.labels(slug) }),
      ]);
    },
  });
}
