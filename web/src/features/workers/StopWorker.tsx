import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Square } from "lucide-react";
import { useId } from "react";
import { keys } from "@/api/keys";
import { stopWorker } from "@/api/workers";
import { Button } from "@/components/button/Button";
import { cn } from "@/lib/cn";
import { ErrorNote } from "./parts";
import { stopFailure } from "./startLead";
import type { StopControl } from "./team";

interface StopWorkerProps {
  slug: string;
  name: string;
  /** What the worker's state allows; the caller shows nothing for a worker that cannot be stopped. */
  control: Exclude<StopControl, null>;
  size: "sm" | "md";
  idleLabel: string;
  busyLabel: string;
  ariaLabel?: string;
  /** The accessible name while pending, where `ariaLabel` replaces the visible label. */
  ariaBusyLabel?: string;
  /** The alignment of the button and its refusal in the column the control sits in. */
  className?: string;
  /** What follows the stop, shown under the button, such as the supervised main staying stopped. */
  note?: string | null;
}

/** The stop button of one worker and the refusal below it. The button stays pending until the worker list shows the daemon's new state. */
export function StopWorker({
  slug,
  name,
  control,
  size,
  idleLabel,
  busyLabel,
  ariaLabel,
  ariaBusyLabel,
  className,
  note,
}: StopWorkerProps) {
  const noteId = useId();
  const queryClient = useQueryClient();
  const stop = useMutation({
    mutationFn: () => stopWorker(slug, name),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: keys.workers(slug) }),
  });
  const pending = stop.isPending || control === "stopping";
  return (
    <div className={cn("flex flex-col gap-6", className)}>
      <Button
        size={size}
        pending={pending}
        pendingLabel={busyLabel}
        icon={<Square aria-hidden="true" size={size === "sm" ? 13 : 15} />}
        aria-label={pending ? ariaBusyLabel : ariaLabel}
        aria-describedby={note ? noteId : undefined}
        onClick={() => {
          stop.mutate();
        }}
      >
        {idleLabel}
      </Button>
      {note ? (
        <p id={noteId} className="text-13 text-text-2">
          {note}
        </p>
      ) : null}
      {stop.isError ? <ErrorNote failure={stopFailure(stop.error)} /> : null}
    </div>
  );
}
