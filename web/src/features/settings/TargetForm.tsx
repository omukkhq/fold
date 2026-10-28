import { useMutation } from "@tanstack/react-query";
import { Plus, Save } from "lucide-react";
import { useState, type SyntheticEvent } from "react";
import {
  addNotifyTarget,
  editNotifyTarget,
  type NotifyScope,
  type NotifyTarget,
  type NotifyTargetAdded,
} from "@/api/notify";
import { Button } from "@/components/button/Button";
import { Checkbox, TextField } from "@/components/form/controls";
import {
  SEGMENT_LIST,
  segmentClasses,
} from "@/components/segments/segmentStyles";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { ErrorNote } from "../workers/parts";
import {
  COLUMN_OPTIONS,
  EVENT_OPTIONS,
  KIND_OPTIONS,
  PRESET_OPTIONS,
  checkDraft,
  choosePreset,
  choosesMoves,
  draftOf,
  failureField,
  newDraft,
  notifyFailure,
  toggleColumn,
  toggleEvent,
  type DraftErrors,
  type TargetDraft,
} from "./notifyModel";

/** The label of a group of choices, in the style of a field's label. */
const FIELD_LABEL =
  "font-title text-13 leading-tight font-semibold text-text-2";

function Segments<T extends string>({
  label,
  options,
  value,
  disabled,
  onChoose,
}: {
  label: string;
  options: readonly { value: T; label: string }[];
  value: T;
  disabled: boolean;
  onChoose: (value: T) => void;
}) {
  return (
    <div className="flex flex-col gap-6">
      <span className={FIELD_LABEL}>{label}</span>
      <div
        role="group"
        aria-label={label}
        className={cn(SEGMENT_LIST, "self-start")}
      >
        {options.map((option) => (
          <button
            key={option.value}
            type="button"
            aria-pressed={value === option.value}
            disabled={disabled}
            onClick={() => {
              onChoose(option.value);
            }}
            className={cn(segmentClasses(value === option.value), focusRing)}
          >
            {option.label}
          </button>
        ))}
      </div>
    </div>
  );
}

/** Adds a target to `scope`, or changes `editing`. It draws no ground of its own: it sits on the surface of the panel or the target that holds it. A change of `scope` keeps what the person typed. Every write stays pending until the settings are read again; the add's body leaves the mutation cache once it is done, as its answer holds a signing secret. */
export function TargetForm({
  scope,
  editing,
  canWrite,
  onSaved,
  onCancel,
}: {
  scope: NotifyScope;
  editing?: NotifyTarget;
  canWrite: boolean;
  onSaved: (added: NotifyTargetAdded | null) => Promise<void>;
  onCancel?: () => void;
}) {
  const [draft, setDraft] = useState<TargetDraft>(() =>
    editing ? draftOf(editing) : newDraft(),
  );
  const [errors, setErrors] = useState<DraftErrors>({});
  const save = useMutation({
    mutationFn: async (draft: TargetDraft) => {
      const checked = checkDraft(draft, editing !== undefined);
      if (checked.errors) {
        return null;
      }
      if ("edit" in checked && editing) {
        await editNotifyTarget(scope, editing.id, checked.edit);
        return null;
      }
      return "add" in checked ? addNotifyTarget(scope, checked.add) : null;
    },
    gcTime: 0,
    onSuccess: async (added) => {
      await onSaved(added);
      if (!editing) {
        setDraft(newDraft(draft.kind));
      }
      save.reset();
    },
  });
  const busy = !canWrite || save.isPending;
  const refusedField = save.isError ? failureField(save.error) : null;
  const refusal = save.isError ? notifyFailure(save.error) : null;
  const submit = (event: SyntheticEvent) => {
    event.preventDefault();
    const checked = checkDraft(draft, editing !== undefined);
    if (checked.errors) {
      setErrors(checked.errors);
      return;
    }
    setErrors({});
    save.mutate(draft);
  };
  const title = editing ? `Edit ${editing.name}` : "Add a target";
  return (
    <form
      noValidate
      aria-label={title}
      onSubmit={submit}
      className="flex flex-col gap-16"
    >
      <h3 className="font-title text-16 leading-tight font-semibold text-text">
        {title}
      </h3>
      <TextField
        label="Name"
        autoComplete="off"
        value={draft.name}
        error={errors.name}
        disabled={busy}
        onChange={(event) => {
          setDraft({ ...draft, name: event.target.value });
        }}
      />
      {editing ? null : (
        <Segments
          label="Type"
          options={KIND_OPTIONS.map((option) => ({
            value: option.kind,
            label: option.label,
          }))}
          value={draft.kind}
          disabled={busy}
          onChoose={(kind) => {
            setDraft({ ...draft, kind });
          }}
        />
      )}
      <TextField
        label="Webhook URL"
        hint={
          editing
            ? "Empty keeps the URL fold has; fold shows only its start."
            : "fold keeps the URL secret and shows only its start after you save."
        }
        autoComplete="off"
        inputMode="url"
        spellCheck={false}
        className="font-mono"
        value={draft.url}
        error={
          errors.url ?? (refusedField === "url" ? refusal?.text : undefined)
        }
        disabled={busy}
        onChange={(event) => {
          setDraft({ ...draft, url: event.target.value });
        }}
      />
      <Segments
        label="Events"
        options={PRESET_OPTIONS.map((option) => ({
          value: option.preset,
          label: option.label,
        }))}
        value={draft.preset}
        disabled={busy}
        onChoose={(preset) => {
          setDraft(choosePreset(draft, preset));
        }}
      />
      <div
        role="group"
        aria-label="Events to send"
        className="grid gap-x-16 gap-y-6 sm:grid-cols-2 lg:grid-cols-3"
      >
        {EVENT_OPTIONS.map((option) => (
          <Checkbox
            key={option.name}
            label={option.label}
            checked={draft.events.includes(option.name)}
            disabled={busy}
            onCheckedChange={(on) => {
              setDraft(toggleEvent(draft, option.name, on));
            }}
          />
        ))}
      </div>
      {errors.events ? (
        <p className="text-13 font-medium text-bad">{errors.events}</p>
      ) : null}
      {choosesMoves(draft) ? (
        <div
          role="group"
          aria-label="Card moves to"
          className="flex flex-col gap-6"
        >
          <span className={FIELD_LABEL}>Card moves to</span>
          <div className="flex flex-wrap gap-x-16 gap-y-6">
            {COLUMN_OPTIONS.map((option) => (
              <Checkbox
                key={option.column}
                label={option.label}
                checked={draft.movesTo.includes(option.column)}
                disabled={busy}
                onCheckedChange={(on) => {
                  setDraft(toggleColumn(draft, option.column, on));
                }}
              />
            ))}
          </div>
        </div>
      ) : null}
      <div className="flex flex-wrap items-end justify-between gap-12">
        <TextField
          label="Grouping window in seconds"
          hint="The notices of this many seconds after the first one go in one send; 0 sends each alone."
          inputMode="numeric"
          value={draft.window}
          error={errors.window}
          disabled={busy}
          onChange={(event) => {
            setDraft({ ...draft, window: event.target.value });
          }}
        />
        <div className="flex gap-8">
          {onCancel ? (
            <Button disabled={save.isPending} onClick={onCancel}>
              Cancel
            </Button>
          ) : null}
          <Button
            type="submit"
            variant={editing ? "quiet" : "primary"}
            disabled={!canWrite}
            pending={save.isPending}
            pendingLabel="Saving…"
            icon={
              editing ? (
                <Save aria-hidden="true" size={16} />
              ) : (
                <Plus aria-hidden="true" size={16} />
              )
            }
          >
            Save
          </Button>
        </div>
      </div>
      {refusal !== null && refusedField === null ? (
        <ErrorNote failure={refusal} />
      ) : null}
    </form>
  );
}
