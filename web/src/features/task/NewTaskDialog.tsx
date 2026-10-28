import { zodResolver } from "@hookform/resolvers/zod";
import * as RadixDialog from "@radix-ui/react-dialog";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowRight, Lightbulb } from "lucide-react";
import { useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { keys } from "@/api/keys";
import { sessionQuery } from "@/api/queries";
import { createTask } from "@/api/tasks";
import { Button } from "@/components/button/Button";
import { FieldHintClass, SENTENCE_HINT } from "@/components/form/Field";
import { TextField } from "@/components/form/controls";
import { NewRequestDialog } from "@/features/requests/NewRequestDialog";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { MentionField } from "@/mentions/MentionField";
import { NO_AREA, areaOptions, createLabels } from "./areas";
import { DialogFrame } from "./DialogFrame";
import { KIND_OPTIONS } from "./humanTask";
import { AreaField, LabelsField, useAreas } from "./LabelFields";
import { submit, WriteError } from "./parts";
import { newTaskSchema, type NewTaskForm } from "./schemas";

export interface NewTaskDialogProps {
  slug: string;
  onClose: () => void;
  /** Opens the created task in the dialog. */
  onCreated: (task: string) => void;
  /** Opens a request made from the hint instead of a task. */
  onRequestCreated: (request: string) => void;
}

/** Points to a request first: a lead plans the tasks of a request, with their areas, dependencies and checks. */
function RequestHint({ onRequest }: { onRequest: () => void }) {
  return (
    <div className="flex flex-wrap items-center gap-x-12 gap-y-4 rounded-m bg-accent-soft px-14 py-10 text-14">
      <Lightbulb
        aria-hidden="true"
        size={16}
        className="shrink-0 text-accent"
      />
      <p className="min-w-0 flex-1">
        Usually a request works better: a lead plans the tasks, and you approve
        the draft.
      </p>
      <button
        type="button"
        onClick={onRequest}
        className={cn(
          "inline-flex shrink-0 items-center gap-6 rounded-s font-title font-semibold text-accent hover:underline",
          focusRing,
        )}
      >
        New request
        <ArrowRight aria-hidden="true" size={14} />
      </button>
    </div>
  );
}

/** The dialog's create form. It stays pending until the daemon names the new task and the board has been read again. */
export function NewTaskDialog({
  slug,
  onClose,
  onCreated,
  onRequestCreated,
}: NewTaskDialogProps) {
  const queryClient = useQueryClient();
  const session = useQuery(sessionQuery());
  const { areas, labels } = useAreas(slug);
  const [requesting, setRequesting] = useState(false);
  const create = useMutation({
    mutationFn: async ({ area, labels: chosen, ...values }: NewTaskForm) => {
      const task = await createTask(slug, {
        ...values,
        labels: createLabels(areas ?? [], area, chosen),
      });
      await queryClient.invalidateQueries({ queryKey: keys.board(slug) });
      return task;
    },
    onSuccess: onCreated,
  });
  const form = useForm<NewTaskForm>({
    resolver: zodResolver(newTaskSchema),
    defaultValues: {
      title: "",
      description: "",
      kind: "proposal",
      area: NO_AREA,
      labels: [],
    },
  });
  return (
    <DialogFrame
      crumb="New task"
      crumbIsId={false}
      described
      narrow
      onClose={onClose}
    >
      <FieldHintClass value={SENTENCE_HINT}>
        <form
          className="flex flex-col gap-20 px-16 pt-16 pb-24 md:px-28 md:pt-22 md:pb-28"
          onSubmit={submit(
            form.handleSubmit((values) => {
              create.mutate(values);
            }),
          )}
        >
          <RequestHint
            onRequest={() => {
              setRequesting(true);
            }}
          />
          <div className="flex flex-col gap-4">
            <RadixDialog.Title className="text-28 leading-tight font-semibold text-text">
              New task
            </RadixDialog.Title>
            <RadixDialog.Description className="max-w-measure text-14 text-text-2">
              fold calculates its status. Add checklists and dependencies once
              it exists.
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
                className="leading-normal"
                ref={field.ref}
                value={field.value}
                onValueChange={field.onChange}
                onBlur={field.onBlur}
              />
            )}
          />
          <div className="grid gap-20 sm:grid-cols-2">
            <Controller
              control={form.control}
              name="area"
              render={({ field }) => (
                <AreaField
                  options={areaOptions(areas ?? [], labels)}
                  value={field.value}
                  onChange={field.onChange}
                  disabled={create.isPending || areas === null}
                />
              )}
            />
            <Controller
              control={form.control}
              name="labels"
              render={({ field }) => (
                <LabelsField
                  labels={labels}
                  areas={areas ?? []}
                  value={field.value}
                  onChange={field.onChange}
                  disabled={create.isPending || areas === null}
                />
              )}
            />
          </div>
          <Controller
            control={form.control}
            name="kind"
            render={({ field }) => (
              <fieldset className="flex flex-col gap-4">
                <legend className="mb-4 font-title text-13 leading-tight font-semibold text-text-2">
                  Who does the work
                </legend>
                {KIND_OPTIONS.map((option) => (
                  <label
                    key={option.value}
                    className={cn(
                      "flex cursor-pointer gap-12 py-8",
                      create.isPending && "cursor-default opacity-60",
                    )}
                  >
                    <input
                      type="radio"
                      name={field.name}
                      value={option.value}
                      checked={field.value === option.value}
                      disabled={create.isPending}
                      onChange={() => {
                        field.onChange(option.value);
                      }}
                      onBlur={field.onBlur}
                      className={cn(
                        "mt-3 size-18 shrink-0 accent-accent",
                        focusRing,
                      )}
                    />
                    <span className="flex min-w-0 flex-col">
                      <span className="font-medium text-text">
                        {option.title}
                      </span>
                      <span className={SENTENCE_HINT}>{option.hint}</span>
                    </span>
                  </label>
                ))}
              </fieldset>
            )}
          />
          <WriteError error={create.error} />
          <div className="flex gap-8">
            <Button
              type="submit"
              variant="primary"
              pending={create.isPending}
              pendingLabel="Creating task…"
            >
              Create task
            </Button>
            <Button
              variant="ghost"
              disabled={create.isPending}
              onClick={onClose}
            >
              Cancel
            </Button>
          </div>
        </form>
      </FieldHintClass>
      {/* Outside the form: a submit inside the request dialog's portal still bubbles through the React tree. */}
      <NewRequestDialog
        slug={slug}
        human={session.data?.human ?? null}
        open={requesting}
        onOpenChange={setRequesting}
        onCreated={onRequestCreated}
      />
    </DialogFrame>
  );
}
