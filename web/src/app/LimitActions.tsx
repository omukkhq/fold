import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Repeat } from "lucide-react";
import { apiErrorCode } from "@/api/errors";
import { keys } from "@/api/keys";
import { sessionQuery } from "@/api/queries";
import { setProjectAccount } from "@/api/workers";
import { Button } from "@/components/button/Button";
import { cn } from "@/lib/cn";
import { accountFailure } from "../features/settings/accountModel";
import { PauseControl } from "./PauseControl";
import { switchLabel, switchPendingLabel } from "./limitActions";

/** The usage-limit line's actions: Pause, and Switch to each other account of the host, which makes it the project's account; fold then runs the waiting turns again on it. A switch stays pending until the daemon answers and Needs you is read again. */
export function LimitActions({
  slug,
  accounts,
  className,
}: {
  slug: string;
  accounts: readonly string[];
  className?: string;
}) {
  const session = useQuery(sessionQuery());
  const queryClient = useQueryClient();
  const write = useMutation({
    mutationFn: (account: string) => setProjectAccount(slug, account),
    onSuccess: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: keys.needsYou(slug) }),
        queryClient.invalidateQueries({ queryKey: keys.workers(slug) }),
      ]),
  });
  const named = (session.data?.human ?? null) !== null;
  const failure = write.isError ? accountFailure(write.error) : null;
  return (
    <div className={cn("flex flex-col items-start gap-6", className)}>
      <div className="flex flex-wrap items-center gap-6">
        <PauseControl slug={slug} className="items-start" />
        {named
          ? accounts.map((account) => (
              <Button
                key={account}
                variant="quiet"
                pending={write.isPending && write.variables === account}
                pendingLabel={switchPendingLabel(account)}
                disabled={write.isPending}
                icon={<Repeat aria-hidden="true" size={16} />}
                onClick={() => {
                  write.mutate(account);
                }}
              >
                {switchLabel(account)}
              </Button>
            ))
          : null}
      </div>
      {failure === null ? null : (
        <span role="alert" className="text-13 text-bad">
          {failure.text}
          {apiErrorCode(write.error) === null ? null : (
            <span className="font-mono text-text-2"> code: {failure.code}</span>
          )}
        </span>
      )}
    </div>
  );
}
