import { useMutation, useQueryClient } from "@tanstack/react-query";
import { FolderPlus } from "lucide-react";
import { keys } from "@/api/keys";
import { restoreProject, type ProjectSummary } from "@/api/projects";
import { Button } from "@/components/button/Button";
import { ErrorNote } from "../workers/parts";
import { addBackFailure } from "./projectsHome";

/** Brings a removed project back into fold; the button stays pending until the daemon answers, and a refusal shows below it. */
export function AddBackProject({ project }: { project: ProjectSummary }) {
  const queryClient = useQueryClient();
  const restore = useMutation({
    mutationFn: () => restoreProject(project.slug),
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: keys.projects() }),
  });
  return (
    <div className="flex flex-col items-end gap-8">
      <Button
        size="sm"
        pending={restore.isPending}
        pendingLabel="Adding back…"
        icon={<FolderPlus aria-hidden="true" size={13} />}
        aria-label={`Add ${project.name} back to fold`}
        onClick={() => {
          restore.mutate();
        }}
      >
        Add back
      </Button>
      {restore.isError ? (
        <ErrorNote failure={addBackFailure(restore.error)} />
      ) : null}
    </div>
  );
}
