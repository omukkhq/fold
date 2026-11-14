import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { keys } from "@/api/keys";
import { sessionQuery } from "@/api/queries";
import { setProjectAccount } from "@/api/workers";
import { Select } from "@/components/form/controls";
import { cn } from "@/lib/cn";
import { accountFailure } from "../features/settings/accountModel";
import { ErrorNote } from "../features/workers/parts";
import { PauseControl } from "./PauseControl";
import {
  SWITCH_ACCOUNT,
  switchItems,
  switchPlaceholder,
} from "./limitActionsModel";

/** The usage-limit line's actions: Pause, and one Switch account dropdown whose options are the other accounts, for any number of them; a choice makes that account the project's at once, and fold then runs the waiting turns again on it. A switch stays pending until the daemon answers and Needs you is read again. */
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
    <div
      className={cn("flex max-w-full flex-col items-start gap-6", className)}
    >
      <div className="flex max-w-full flex-wrap items-center gap-6">
        <PauseControl slug={slug} className="items-start" />
        {items === null ? null : (
          // The closed select is as wide as its longest option, so on a phone it stops at the column and cuts its text.
          <div className="max-w-full min-w-0">
            <Select
              label={SWITCH_ACCOUNT}
              labelHidden
              // The first option stays the value, so the dropdown never names a target nobody chose, also after a refusal.
              value=""
              disabled={write.isPending}
              className="w-auto max-w-full truncate py-7 font-title text-14 leading-tight font-semibold"
              onChange={(event) => {
                if (event.target.value !== "") {
                  write.mutate(event.target.value);
                }
              }}
            >
              <option value="" disabled>
                {switchPlaceholder(pendingFor)}
              </option>
              {items.map((item) => (
                <option key={item.account} value={item.account}>
                  {item.label}
                </option>
              ))}
            </Select>
          </div>
        )}
      </div>
      {failure === null ? null : <ErrorNote failure={failure} />}
    </div>
  );
}
