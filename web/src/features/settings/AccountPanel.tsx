import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { apiErrorCode } from "@/api/errors";
import { keys } from "@/api/keys";
import { workersQuery } from "@/api/queries";
import { setProjectAccount } from "@/api/workers";
import { Button } from "@/components/button/Button";
import { FieldHintClass, SENTENCE_HINT } from "@/components/form/Field";
import { Select } from "@/components/form/controls";
import { FailurePanel } from "@/components/states/FailurePanel";
import { Skeleton } from "@/components/states/Skeleton";
import { ErrorNote } from "../workers/parts";
import {
  accountFailure,
  accountHint,
  accountOptions,
  shownValue,
} from "./accountModel";

/** The Claude account of the project's workers: a choice saves at once and stays pending until the daemon answers and the list is read again. */
export function AccountPanel({
  slug,
  canWrite,
  className,
}: {
  slug: string;
  canWrite: boolean;
  className: string;
}) {
  const workers = useQuery(workersQuery(slug));
  const client = useQueryClient();
  const save = useMutation({
    mutationFn: (account: string) => setProjectAccount(slug, account),
    // A refusal such as `account_not_found` also reads the list again, so the select offers the accounts as they are now.
    onSettled: () => client.invalidateQueries({ queryKey: keys.workers(slug) }),
  });
  if (workers.isError) {
    return (
      <FailurePanel
        title="Could not load the project's account"
        text={accountFailure(workers.error).text}
        code={apiErrorCode(workers.error)}
        action={
          <Button
            icon={<RefreshCw aria-hidden="true" size={16} />}
            onClick={() => void workers.refetch()}
          >
            Try again
          </Button>
        }
      />
    );
  }
  if (workers.data === undefined) {
    return <Skeleton />;
  }
  const view = workers.data.account;
  const value = shownValue(view, save.isPending ? save.variables : undefined);
  return (
    <div className={className}>
      <FieldHintClass value={SENTENCE_HINT}>
        <Select
          label="Account"
          hint={accountHint(view, save.isPending)}
          value={value}
          disabled={!canWrite || save.isPending}
          onChange={(event) => {
            save.mutate(event.target.value);
          }}
        >
          {accountOptions(view).map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </Select>
      </FieldHintClass>
      {save.isError ? <ErrorNote failure={accountFailure(save.error)} /> : null}
    </div>
  );
}
