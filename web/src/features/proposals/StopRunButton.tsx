import { useMutation } from "@tanstack/react-query";
import { Square } from "lucide-react";
import { useState } from "react";
import { cancelCheckRun } from "@/api/proposals";
import { Button } from "@/components/button/Button";
import { FailurePanel } from "@/components/states/FailurePanel";
import { stopFailure, stopPending } from "./stopRun";

/** Stop for one running check run. The button stays pending until the run shows as ended, which the `checks.finished` event brings; this component unmounts then. */
export function StopRunButton({ slug, run }: { slug: string; run: string }) {
  const [accepted, setAccepted] = useState(false);
  const stop = useMutation({
    mutationFn: () => cancelCheckRun(slug, run),
    onMutate: () => {
      setAccepted(false);
    },
    onSuccess: () => {
      setAccepted(true);
    },
  });
  const failure = stop.isError ? stopFailure(stop.error) : null;
  return (
    <div className="flex flex-col gap-6 px-6">
      <Button
        size="sm"
        pending={stopPending({ sending: stop.isPending, accepted })}
        pendingLabel="Stopping"
        icon={<Square aria-hidden="true" size={14} />}
        aria-label={`Stop ${run}`}
        onClick={() => {
          stop.mutate();
        }}
      >
        Stop
      </Button>
      {failure ? (
        <FailurePanel
          title="fold did not stop the run"
          text={failure.text}
          code={failure.code}
        />
      ) : null}
    </div>
  );
}
