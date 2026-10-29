import * as RadixDialog from "@radix-ui/react-dialog";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Ellipsis, FolderMinus, X } from "lucide-react";
import { useState } from "react";
import { keys } from "@/api/keys";
import { removeProject, type ProjectSummary } from "@/api/projects";
import { InlineCode } from "@/components/code/CodeText";
import { Button, IconButton } from "@/components/button/Button";
import { Mono } from "@/components/mono/Mono";
import { Menu } from "@/components/overlay/Menu";
import { cn } from "@/lib/cn";
import {
  dialogBody,
  dialogContent,
  dialogOverlay,
} from "@/components/overlay/dialogStyles";
import { ErrorNote } from "../workers/parts";
import { removeFailure } from "./projectsHome";

/** The project's row menu, whose Remove opens the dialog that asks first and says how the project comes back. The confirm stays pending until the daemon answers. */
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
    <>
      <Menu
        align="end"
        items={[
          {
            label: "Remove from fold…",
            icon: FolderMinus,
            danger: true,
            onSelect: () => {
              setOpen(true);
            },
          },
        ]}
        trigger={
          <IconButton variant="ghost" aria-label={`More for ${project.name}`}>
            <Ellipsis aria-hidden="true" size={16} />
          </IconButton>
        }
      />
      <RadixDialog.Root
        open={open}
        onOpenChange={(next) => {
          if (next) setOpen(true);
          else close();
        }}
      >
        <RadixDialog.Portal>
          <RadixDialog.Overlay className={dialogOverlay}>
            <RadixDialog.Content
              className={cn(dialogContent, "w-request-dialog")}
            >
              <div className="flex shrink-0 items-center justify-between gap-12 pt-18 pr-16 pb-6 pl-24">
                <RadixDialog.Title className="min-w-0 text-21 leading-tight font-semibold text-text">
                  Remove {project.name} from fold?
                </RadixDialog.Title>
                <RadixDialog.Close asChild>
                  <IconButton
                    aria-label="Close"
                    variant="ghost"
                    disabled={remove.isPending}
                  >
                    <X aria-hidden="true" size={16} />
                  </IconButton>
                </RadixDialog.Close>
              </div>
              <div className={dialogBody}>
                <div className="flex flex-col gap-12 px-24 pt-8 pb-4">
                  <RadixDialog.Description className="max-w-measure text-15 text-text">
                    fold closes the project and stops showing it. Nothing in the
                    folder is deleted: its data stays in .fold. fold deletes the
                    project's notice targets and their secrets.
                  </RadixDialog.Description>
                  <Mono className="rounded-s bg-sunken px-12 py-8 text-13 break-all text-text">
                    {project.root}
                  </Mono>
                  <p className="max-w-measure text-15 text-text-2">
                    To bring it back, run <InlineCode>fld claude</InlineCode>,{" "}
                    <InlineCode>fld new</InlineCode> or{" "}
                    <InlineCode>fld init</InlineCode> in that folder, or search
                    for it here and choose Add back.
                  </p>
                  {remove.isError ? (
                    <ErrorNote failure={removeFailure(remove.error)} />
                  ) : null}
                </div>
              </div>
              <div className="flex shrink-0 items-center justify-end gap-8 px-24 pt-18 pb-20">
                <Button disabled={remove.isPending} onClick={close}>
                  Cancel
                </Button>
                <Button
                  variant="danger"
                  pending={remove.isPending}
                  pendingLabel="Removing…"
                  icon={<FolderMinus aria-hidden="true" size={16} />}
                  onClick={() => {
                    remove.mutate();
                  }}
                >
                  Remove from fold
                </Button>
              </div>
            </RadixDialog.Content>
          </RadixDialog.Overlay>
        </RadixDialog.Portal>
      </RadixDialog.Root>
    </>
  );
}
