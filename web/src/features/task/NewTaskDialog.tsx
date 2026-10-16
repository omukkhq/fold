import { zodResolver } from "@hookform/resolvers/zod";
import * as RadixDialog from "@radix-ui/react-dialog";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Controller, useForm } from "react-hook-form";
import { keys } from "@/api/keys";
import { createTask } from "@/api/tasks";
import { Button } from "@/components/button/Button";
import { Checkbox, TextField } from "@/components/form/controls";
import { MentionField } from "@/mentions/MentionField";
import { DialogFrame } from "./DialogFrame";
import { submit, WriteError } from "./parts";
import { newTaskSchema, type NewTaskForm } from "./schemas";

export interface NewTaskDialogProps {
  slug: string;
  onClose: () => void;
  /** Opens the created task in the dialog. */
  onCreated: (task: string) => void;
}

/** The dialog's create form. It stays pending until the daemon names the new task and the board has been read again. */
export function NewTaskDialog({
  slug,
  onClose,
  onCreated,
}: NewTaskDialogProps) {
  const queryClient = useQueryClient();
  const create = useMutation({
    mutationFn: async (values: NewTaskForm) => {
      const task = await createTask(slug, values);
      await queryClient.invalidateQueries({ queryKey: keys.board(slug) });
      return task;
    },
    onSuccess: onCreated,
  });
  const form = useForm<NewTaskForm>({
    resolver: zodResolver(newTaskSchema),
    defaultValues: { title: "", description: "", noProposal: false },
  });
  return (
    <DialogFrame crumb="New task" crumbIsId={false} described onClose={onClose}>
      <form
        className="flex flex-col gap-16 px-24 pt-22 pb-24"
        onSubmit={submit(
          form.handleSubmit((values) => {
            create.mutate(values);
          }),
        )}
      >
        <div className="flex flex-col gap-4">
          <RadixDialog.Title className="text-page leading-snug font-semibold text-text">
            New task
          </RadixDialog.Title>
          <RadixDialog.Description className="text-bar text-text-muted">
            fold calculates its status. Add labels, checklists and dependencies
            once it exists.
          </RadixDialog.Description>
        </div>
        <TextField
          label="Title"
          autoFocus
          disabled={create.isPending}
          error={form.formState.errors.title?.message}
          {...form.register("title")}
        />
        <Controller
          control={form.control}
          name="description"
          render={({ field }) => (
            <MentionField
              slug={slug}
              label="Description"
              hint="Markdown, with tables and task lists. HTML is not shown. Type # for an item and @ for a file."
              rows={8}
              disabled={create.isPending}
              error={form.formState.errors.description?.message}
              className="font-mono text-meta"
              ref={field.ref}
              value={field.value}
              onValueChange={field.onChange}
              onBlur={field.onBlur}
            />
          )}
        />
        <Controller
          control={form.control}
          name="noProposal"
          render={({ field }) => (
            <Checkbox
              label="Needs no proposal"
              hint="For work that changes no project file, such as screens drawn through an MCP server. The coder reports the work, and a lead check completes it."
              disabled={create.isPending}
              ref={field.ref}
              checked={field.value}
              onCheckedChange={field.onChange}
              onBlur={field.onBlur}
            />
          )}
        />
        <WriteError error={create.error} />
        <div className="flex gap-8">
          <Button type="submit" variant="primary" pending={create.isPending}>
            Create task
          </Button>
          <Button variant="ghost" disabled={create.isPending} onClick={onClose}>
            Cancel
          </Button>
        </div>
      </form>
    </DialogFrame>
  );
}
