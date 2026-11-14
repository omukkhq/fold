import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronDown, Repeat } from "lucide-react";
import { apiErrorCode } from "@/api/errors";
import { keys } from "@/api/keys";
import { sessionQuery } from "@/api/queries";
import { setProjectAccount } from "@/api/workers";
import { Button } from "@/components/button/Button";
import { Menu } from "@/components/overlay/Menu";
import { cn } from "@/lib/cn";
import { accountFailure } from "../features/settings/accountModel";
import { PauseControl } from "./PauseControl";
import {
  SWITCH_MENU_HINT,
  switchItems,
  switchPendingLabel,
} from "./limitActionsModel";

/** The usage-limit line's actions: Pause, and one Switch account menu whose items are the other accounts, for any number of them; a choice makes that account the project's, and fold then runs the waiting turns again on it. A switch stays pending until the daemon answers and Needs you is read again. */
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
    // A refusal such as `account_not_found` also reads the item again, so its switches name the accounts as they are now.
    onSettled: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: keys.needsYou(slug) }),
        queryClient.invalidateQueries({ queryKey: keys.workers(slug) }),
      ]),
  });
  const named = (session.data?.human ?? null) !== null;
  const items = named ? switchItems(accounts) : null;
  const failure = write.isError ? accountFailure(write.error) : null;
  const pendingFor = write.isPending ? write.variables : undefined;
  return (
    <div className={cn("flex flex-col items-start gap-6", className)}>
      <div className="flex flex-wrap items-center gap-6">
        <PauseControl slug={slug} className="items-start" />
        {items === null ? null : (
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
                <ChevronDown
                  aria-hidden="true"
                  size={14}
                  className="shrink-0 text-text-2"
                />
              </Button>
            }
            items={items.map((item) => ({
              label: item.label,
              onSelect: () => {
                write.mutate(item.account);
              },
              disabled: write.isPending,
            }))}
          />
        )}
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
