import {
  useMutation,
  useMutationState,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { Pause, Play } from "lucide-react";
import { keys } from "@/api/keys";
import { pauseProject, resumeProject } from "@/api/projects";
import { projectQuery, sessionQuery } from "@/api/queries";
import { Button } from "@/components/button/Button";
import { failureText } from "@/format/failure";
import { cn } from "@/lib/cn";
import {
  lastPauseWrite,
  pauseControl,
  pauseWrites,
  type PauseAction,
} from "./pause";

/** The Agents panel's pause and resume of every agent of the project. Only a named human pauses, so the control waits for a name; it stays pending until the project shows the daemon's new state. */
export function PauseControl({
  slug,
  className,
  onSunken = false,
}: {
  slug: string;
  className?: string;
  /** The control sits on the sunken tone, as in the Agents panel, so the button takes the surface instead of the fill. */
  onSunken?: boolean;
}) {
  const project = useQuery(projectQuery(slug));
  const session = useQuery(sessionQuery());
  const queryClient = useQueryClient();
  // The write stays pending until the project is read again, so the control never shows success before the daemon's state does.
  const mutationKey = pauseWrites(slug);
  const write = useMutation({
    mutationKey,
    mutationFn: (action: PauseAction) =>
      action === "pause" ? pauseProject(slug) : resumeProject(slug),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: keys.project(slug) }),
  });
  const last = lastPauseWrite(
    useMutationState({ filters: { mutationKey }, select: (m) => m.state }),
  );
  const overview = project.data?.overview ?? null;
  const view = pauseControl(
    overview === null ? undefined : overview.paused_since,
    last.sent,
  );
  if (view === null || (session.data?.human ?? null) === null) {
    return null;
  }
  const Icon = view.paused ? Play : Pause;
  return (
    <div className={cn("flex flex-col items-end gap-6", className)}>
      <Button
        variant="quiet"
        pending={view.pending}
        pendingLabel={view.pendingLabel}
        icon={<Icon aria-hidden="true" size={16} />}
        className={cn(
          onSunken && "bg-surface hover:bg-surface",
          view.paused && "bg-warn-soft text-warn hover:bg-warn-soft",
        )}
        onClick={() => {
          write.mutate(view.action);
        }}
      >
        {view.label}
      </Button>
      {last.error === null ? null : (
        <span role="alert" className="text-13 text-bad">
          {failureText(last.error)}
        </span>
      )}
    </div>
  );
}
