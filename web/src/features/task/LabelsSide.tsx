import { zodResolver } from "@hookform/resolvers/zod";
import { CircleAlert, Pencil, Plus } from "lucide-react";
import { useIsMutating } from "@tanstack/react-query";
import { useForm } from "react-hook-form";
import type { BoardLabel } from "@/api/board";
import {
  attachLabel,
  createLabelOn,
  removeLabel,
  setLabels,
  type TaskDetail,
} from "@/api/tasks";
import type { AreaEntry } from "@/api/workers";
import { Button } from "@/components/button/Button";
import { Select, TextField } from "@/components/form/controls";
import { Label } from "@/components/label/Label";
import { LABEL_NAMES } from "@/components/label/labelColors";
import { Popover } from "@/components/overlay/Popover";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import {
  areaChanges,
  areaOptions,
  labelChoices,
  labelControls,
  leadText,
  markedArea,
  swapArea,
  taskLabelView,
} from "./areas";
import { fieldError } from "./errors";
import { AreaMenu, RemovableLabel, useAreas } from "./LabelFields";
import { SideGroup, SmallAction, submit, WriteError } from "./parts";
import { labelSchema, type LabelForm } from "./schemas";
import { useTaskWrite } from "./useTaskWrite";

interface GroupProps {
  slug: string;
  task: TaskDetail;
  areas: readonly AreaEntry[];
  labels: readonly BoardLabel[];
  /** Which controls take input, from `labelControls`. */
  controls: { area: boolean; labels: boolean };
}

/** Every label write of one task shares this key, so the groups wait for each other's writes. */
function labelWrites(slug: string, task: string) {
  return ["label-write", slug, task] as const;
}

/** The task's area and its lead, and the one control that changes it: the whole label set is written at once, so the task never holds two areas. */
function AreaGroup({ slug, task, areas, labels, controls }: GroupProps) {
  const view = taskLabelView(task.labels, areas);
  const write = useTaskWrite(
    slug,
    task.task,
    (next: string) =>
      setLabels(slug, task.task, swapArea(task.labels, areas, next)),
    labelWrites(slug, task.task),
  );
  return (
    <SideGroup
      title="Area"
      action={
        <AreaMenu
          options={areaOptions(areas, labels)}
          value={markedArea(view, task.areas_conflict)}
          onValueChange={(next) => {
            if (areaChanges(view, task.areas_conflict, next))
              write.mutate(next);
          }}
        >
          <SmallAction
            icon={Pencil}
            pending={write.isPending}
            disabled={!controls.area}
            pendingLabel="Changing area…"
            className="text-primary-text"
          >
            Change
          </SmallAction>
        </AreaMenu>
      }
    >
      {task.areas_conflict ? (
        <p
          role="note"
          className="flex items-start gap-10 rounded-m bg-warning-soft px-12 py-10 text-meta leading-normal text-text-muted"
        >
          <CircleAlert
            aria-hidden="true"
            size={16}
            className="mt-2 shrink-0 text-warning"
          />
          This task has the labels of two areas, but a task belongs to one. Use
          Change to keep one area; fold refuses other label changes until then.
        </p>
      ) : null}
      {view.areas.length === 0 ? (
        <p className="flex items-start gap-10 rounded-m bg-warning-soft px-12 py-10 text-meta leading-normal text-text-muted">
          <CircleAlert
            aria-hidden="true"
            size={16}
            className="mt-2 shrink-0 text-warning"
          />
          No area yet: the main agent chooses one. Pick an area to give it to
          that lead at once.
        </p>
      ) : (
        <ul aria-label="Areas of this task" className="flex flex-col gap-6">
          {view.areas.map((area) => (
            <li key={area.name} className="flex items-center gap-8">
              <Label color={area.label.color}>{area.label.name}</Label>
              <span className="text-meta text-text-faint">
                lead: {leadText(area.lead)}
              </span>
            </li>
          ))}
        </ul>
      )}
      <WriteError error={write.error} />
    </SideGroup>
  );
}

/** The project's labels that are no area and not on the task, each added with one write, and a form that creates a label on the task. */
function AddLabelPanel({ slug, task, areas, labels, controls }: GroupProps) {
  const attach = useTaskWrite(
    slug,
    task.task,
    (name: string) => attachLabel(slug, task.task, name),
    labelWrites(slug, task.task),
  );
  const create = useTaskWrite(
    slug,
    task.task,
    (label: LabelForm) => createLabelOn(slug, task.task, label),
    labelWrites(slug, task.task),
  );
  const form = useForm<LabelForm>({
    resolver: zodResolver(labelSchema),
    defaultValues: { name: "", color: "sage" },
  });
  const choices = labelChoices(
    labels,
    areas,
    task.labels.map((label) => label.name),
    "",
  );
  return (
    <div className="flex w-search flex-col gap-12">
      {choices.length > 0 ? (
        <ul aria-label="Labels of the project" className="flex flex-col gap-2">
          {choices.map((label) => (
            <li key={label.name}>
              <SmallAction
                icon={Plus}
                aria-label={`Add label ${label.name}`}
                pending={attach.isPending && attach.variables === label.name}
                pendingLabel={`Adding ${label.name}…`}
                disabled={!controls.labels}
                className="w-full text-bar text-text"
                onClick={() => {
                  attach.mutate(label.name);
                }}
              >
                {label.name}
              </SmallAction>
            </li>
          ))}
        </ul>
      ) : null}
      <WriteError error={attach.error} />
      <form
        aria-label="Create a label"
        className="flex flex-col gap-8"
        onSubmit={submit(
          form.handleSubmit((values) => {
            create.mutate(values, {
              onSuccess: () => {
                form.reset({ name: "", color: values.color });
              },
            });
          }),
        )}
      >
        <TextField
          label="New label name"
          disabled={create.isPending}
          error={
            form.formState.errors.name?.message ??
            fieldError(create.error, "name")
          }
          {...form.register("name")}
        />
        <Select
          label="Colour"
          disabled={create.isPending}
          error={
            form.formState.errors.color?.message ??
            fieldError(create.error, "color")
          }
          {...form.register("color")}
        >
          {LABEL_NAMES.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </Select>
        <WriteError error={create.error} />
        <Button
          type="submit"
          variant="primary"
          pending={create.isPending}
          pendingLabel="Creating label…"
          disabled={!controls.labels}
        >
          Create label
        </Button>
      </form>
    </div>
  );
}

/** The task's labels that are no area, each with its remove button, and the picker that adds one. */
function LabelsGroup(props: GroupProps) {
  const { slug, task, areas, controls } = props;
  const remove = useTaskWrite(
    slug,
    task.task,
    (name: string) => removeLabel(slug, task.task, name),
    labelWrites(slug, task.task),
  );
  const { others } = taskLabelView(task.labels, areas);
  return (
    <SideGroup title="Labels">
      <div className="flex flex-wrap items-center gap-6">
        {others.map((label) => (
          <RemovableLabel
            key={label.name}
            label={label}
            pending={remove.isPending && remove.variables === label.name}
            disabled={!controls.labels}
            onRemove={() => {
              remove.mutate(label.name);
            }}
          />
        ))}
        <Popover
          trigger={
            <button
              type="button"
              disabled={!controls.labels}
              className={cn(
                "inline-flex items-center gap-4 rounded-s border border-dashed border-border bg-surface px-7 py-2 text-label leading-snug font-medium text-text-muted hover:bg-sunken disabled:cursor-not-allowed disabled:opacity-60",
                focusRing,
              )}
            >
              <Plus aria-hidden="true" size={11} />
              Add label
            </button>
          }
        >
          <AddLabelPanel {...props} />
        </Popover>
      </div>
      <WriteError error={remove.error} />
    </SideGroup>
  );
}

/** The side column's Area and Labels groups. They wait for the area list, since it decides which labels are areas. */
export function LabelsSide({ slug, task }: { slug: string; task: TaskDetail }) {
  const { areas, labels, failed, retry } = useAreas(slug);
  const writing =
    useIsMutating({ mutationKey: labelWrites(slug, task.task) }) > 0;
  const controls = labelControls(task.areas_conflict, writing);
  if (failed) {
    return (
      <SideGroup title="Area">
        <ErrorBanner
          className="rounded-m border-b-0 px-12 inset-ring inset-ring-border"
          title="The areas could not load."
          message="The daemon did not answer."
          onRetry={retry}
        />
      </SideGroup>
    );
  }
  if (areas === null) {
    return (
      <SideGroup title="Area">
        <p className="text-bar text-text-muted">Reading the areas…</p>
      </SideGroup>
    );
  }
  return (
    <>
      <AreaGroup
        slug={slug}
        task={task}
        areas={areas}
        labels={labels}
        controls={controls}
      />
      <LabelsGroup
        slug={slug}
        task={task}
        areas={areas}
        labels={labels}
        controls={controls}
      />
    </>
  );
}
