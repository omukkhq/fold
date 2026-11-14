import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronDown, Repeat } from "lucide-react";
import { keys } from "@/api/keys";
import { sessionQuery } from "@/api/queries";
import { setProjectAccount } from "@/api/workers";
import { Button } from "@/components/button/Button";
import { Menu } from "@/components/overlay/Menu";
import { cn } from "@/lib/cn";
import { accountFailure } from "../features/settings/accountModel";
import { ErrorNote } from "../features/workers/parts";
import { PauseControl } from "./PauseControl";
import {
  SWITCH_ACCOUNT,
  switchItems,
  switchPendingLabel,
} from "./limitActionsModel";

/** The usage-limit line's actions: Pause, and one Switch account control for any number of other accounts, whose list makes the chosen account the project's at once; fold then runs the waiting turns again on it. A switch stays pending until the daemon answers and Needs you is read again. */
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
    // A refusal such as `account_not_found` also reads the item again, so its list names the accounts as they are now.
    onSettled: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: keys.needsYou(slug) }),
        queryClient.invalidateQueries({ queryKey: keys.workers(slug) }),
      ]),
  });
  const named = (session.data?.human ?? null) !== null;
  const items = named ? switchItems(accounts) : null;
  const failure = write.isError ? accountFailure(write.error) : null;
  return (
    <div
      className={cn("flex max-w-full flex-col items-start gap-6", className)}
    >
      <div className="flex max-w-full flex-wrap items-center gap-6">
        <PauseControl slug={slug} className="items-start" />
        {items === null ? null : (
          <Menu
            minTriggerWidth
            trigger={
              <Button
                variant="quiet"
                pending={write.isPending}
                pendingLabel={switchPendingLabel(write.variables ?? "")}
                icon={<Repeat aria-hidden="true" size={16} />}
                className="w-switch-account max-w-full min-w-0 overflow-hidden"
              >
                <span className="min-w-0 flex-1 truncate text-left">
                  {SWITCH_ACCOUNT}
                </span>
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
            }))}
          />
        )}
      </div>
      {failure === null ? null : <ErrorNote failure={failure} />}
    </div>
  );
}
