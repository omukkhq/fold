import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeftRight, ChevronDown, Repeat } from "lucide-react";
import { useState } from "react";
import { keys } from "@/api/keys";
import { sessionQuery } from "@/api/queries";
import { setProjectAccount } from "@/api/workers";
import { Button, IconButton } from "@/components/button/Button";
import { Menu } from "@/components/overlay/Menu";
import { cn } from "@/lib/cn";
import { accountFailure } from "../features/settings/accountModel";
import { ErrorNote } from "../features/workers/parts";
import { PauseControl } from "./PauseControl";
import {
  chosenTarget,
  switchButtonLabel,
  switchItems,
  switchPendingLabel,
  triggerLabel,
} from "./limitActionsModel";

/** The usage-limit line's actions: Pause, and one Switch account control for any number of other accounts. Its list only picks an account; the switch button beside it makes that account the project's, and fold then runs the waiting turns again on it. A switch stays pending until the daemon answers and Needs you is read again, and the choice resets after it. */
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
  const [picked, setPicked] = useState<string | null>(null);
  const write = useMutation({
    mutationFn: (account: string) => setProjectAccount(slug, account),
    // A refusal such as `account_not_found` also reads the item again, so its list names the accounts as they are now.
    onSettled: async () => {
      setPicked(null);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: keys.needsYou(slug) }),
        queryClient.invalidateQueries({ queryKey: keys.workers(slug) }),
      ]);
    },
  });
  const named = (session.data?.human ?? null) !== null;
  const items = named ? switchItems(accounts) : null;
  const chosen = chosenTarget(picked, accounts);
  const failure = write.isError ? accountFailure(write.error) : null;
  const pending = write.isPending;
  return (
    <div
      className={cn("flex max-w-full flex-col items-start gap-6", className)}
    >
      <div className="flex max-w-full flex-wrap items-center gap-6">
        <PauseControl slug={slug} className="items-start" />
        {items === null ? null : (
          <div className="flex max-w-full min-w-0 items-center gap-2">
            <Menu
              matchTriggerWidth
              trigger={
                <Button
                  variant="quiet"
                  pending={pending}
                  pendingLabel={switchPendingLabel(write.variables ?? "")}
                  icon={<Repeat aria-hidden="true" size={16} />}
                  className="min-w-0 rounded-r-none"
                >
                  <span className="min-w-0 truncate">
                    {triggerLabel(chosen)}
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
                  setPicked(item.account);
                },
              }))}
            />
            <IconButton
              variant="quiet"
              aria-label={switchButtonLabel(chosen)}
              disabled={chosen === null || pending}
              className="shrink-0 rounded-l-none"
              onClick={() => {
                if (chosen !== null) {
                  write.mutate(chosen);
                }
              }}
            >
              <ArrowLeftRight aria-hidden="true" size={16} />
            </IconButton>
          </div>
        )}
      </div>
      {failure === null ? null : <ErrorNote failure={failure} />}
    </div>
  );
}
