import * as RadixDialog from "@radix-ui/react-dialog";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { FolderMinus, X } from "lucide-react";
import { useState } from "react";
import { keys } from "@/api/keys";
import { removeProject, type ProjectSummary } from "@/api/projects";
import { InlineCode } from "@/components/code/CodeText";
import { Button, IconButton } from "@/components/button/Button";
import { Mono } from "@/components/mono/Mono";
import { ErrorNote } from "../workers/parts";
import { removeFailure } from "./projectsHome";

/** The button that takes a project off fold, and the dialog that asks first and says how the project comes back. The confirm stays pending until the daemon answers. */
export function RemoveProject({ project }: { project: ProjectSummary }) {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const remove = useMutation({
    mutationFn: () => removeProject(project.slug),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: keys.projects() });
      setOpen(false);
    },
  });
  const close = () => {
    if (remove.isPending) return;
    remove.reset();
    setOpen(false);
  };
  return (
    <RadixDialog.Root
      open={open}
      onOpenChange={(next) => {
        if (next) setOpen(true);
        else close();
      }}
    >
      <RadixDialog.Trigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className="text-danger hover:bg-danger-soft"
          icon={<FolderMinus aria-hidden="true" size={13} />}
          aria-label={`Remove ${project.name} from fold`}
        >
          Remove
        </Button>
      </RadixDialog.Trigger>
      <RadixDialog.Portal>
        <RadixDialog.Overlay className="fixed inset-0 bg-scrim" />
        <RadixDialog.Content className="fixed top-1/2 left-1/2 flex max-h-full w-request-dialog max-w-full -translate-x-1/2 -translate-y-1/2 flex-col overflow-y-auto rounded-l bg-raised shadow-dialog inset-ring inset-ring-border outline-none">
          <div className="flex items-center justify-between gap-12 border-b border-border-subtle py-12 pr-16 pl-24">
            <RadixDialog.Title className="min-w-0 truncate text-section leading-snug font-semibold text-text">
              Remove {project.name} from fold?
            </RadixDialog.Title>
            <RadixDialog.Close asChild>
              <IconButton
                aria-label="Close"
                size="sm"
                disabled={remove.isPending}
              >
                <X aria-hidden="true" size={13} />
              </IconButton>
            </RadixDialog.Close>
          </div>
          <div className="flex flex-col gap-12 px-24 pt-18 pb-20">
            <RadixDialog.Description className="text-bar text-text-muted">
              fold closes the project and stops showing it. Nothing in the
              folder is deleted: its data stays in .fold. fold deletes the
              project's notification targets and their secrets.
            </RadixDialog.Description>
            <Mono className="text-meta break-all text-text">
              {project.root}
            </Mono>
            <p className="text-bar text-text-muted">
              To bring it back, run <InlineCode>fld claude</InlineCode>,{" "}
              <InlineCode>fld new</InlineCode> or{" "}
              <InlineCode>fld init</InlineCode> in that folder, or search for it
              here and choose Add back.
            </p>
            {remove.isError ? (
              <ErrorNote failure={removeFailure(remove.error)} />
            ) : null}
          </div>
          <div className="flex items-center justify-end gap-8 border-t border-border-subtle px-24 py-12">
            <Button disabled={remove.isPending} onClick={close}>
              Cancel
            </Button>
            <Button
              variant="danger"
              pending={remove.isPending}
              pendingLabel="Removing…"
              icon={<FolderMinus aria-hidden="true" size={15} />}
              onClick={() => {
                remove.mutate();
              }}
            >
              Remove from fold
            </Button>
          </div>
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}
