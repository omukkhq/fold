import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Repeat } from "lucide-react";
import { apiErrorCode } from "@/api/errors";
import { keys } from "@/api/keys";
import { sessionQuery } from "@/api/queries";
import { setProjectAccount } from "@/api/workers";
import { Button } from "@/components/button/Button";
import { Menu } from "@/components/overlay/Menu";
import { cn } from "@/lib/cn";
import {
  accountFailure,
  accountLabel,
} from "../features/settings/accountModel";
import { PauseControl } from "./PauseControl";
import {
  SWITCH_MENU_HINT,
  switchLabel,
  switchLayout,
  switchPendingLabel,
} from "./limitSwitch";

/** The usage-limit line's actions: Pause, and a switch to each other account, which makes it the project's account; fold then runs the waiting turns again on it. Up to two targets show as buttons, more as one Switch account menu. A switch stays pending until the daemon answers and Needs you is read again. */
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
  const layout = named ? switchLayout(accounts.length) : "none";
  const failure = write.isError ? accountFailure(write.error) : null;
  const pendingFor = write.isPending ? write.variables : undefined;
  return (
    <div className={cn("flex flex-col items-start gap-6", className)}>
      <div className="flex flex-wrap items-center gap-6">
        <PauseControl slug={slug} className="items-start" />
        {layout === "buttons"
          ? accounts.map((account) => (
              <Button
                key={account}
                variant="quiet"
                pending={pendingFor === account}
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
        {layout === "menu" ? (
          <Menu
            hint={SWITCH_MENU_HINT}
            trigger={
              <Button
                variant="quiet"
                pending={pendingFor !== undefined}
                pendingLabel={switchPendingLabel(pendingFor ?? "")}
                icon={<Repeat aria-hidden="true" size={16} />}
              >
                Switch account
              </Button>
            }
            items={accounts.map((account) => ({
              label: accountLabel(account),
              onSelect: () => {
                write.mutate(account);
              },
              disabled: write.isPending,
            }))}
          />
        ) : null}
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
