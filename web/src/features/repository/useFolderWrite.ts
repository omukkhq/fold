import { useMutation, useQueryClient } from "@tanstack/react-query";
import { keys } from "@/api/keys";

/**
 * A human write to the project folder or its stash. It stays pending until the stash, Needs you and the repository have been read again, after a refusal too, so the screen never shows a result the daemon has not confirmed and a refusal is read next to the state that caused it.
 */
export function useFolderWrite<V>(
  slug: string,
  write: (vars: V) => Promise<unknown>,
) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationKey: [slug, "folder-write"],
    mutationFn: async (vars: V) => {
      try {
        await write(vars);
      } finally {
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: keys.stash(slug) }),
          queryClient.invalidateQueries({ queryKey: keys.needsYou(slug) }),
          queryClient.invalidateQueries({ queryKey: keys.repository(slug) }),
        ]);
      }
    },
  });
}
