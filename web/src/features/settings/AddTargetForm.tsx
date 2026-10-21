import { useMutation } from "@tanstack/react-query";
import { KeyRound, Plus } from "lucide-react";
import { useState, type SyntheticEvent } from "react";
import {
  addNotifyTarget,
  type NewNotifyTarget,
  type NotifyScope,
} from "@/api/notify";
import { Button } from "@/components/button/Button";
import { Select, TextField } from "@/components/form/controls";
import {
  SEGMENT_LIST,
  segmentClasses,
} from "@/components/segments/segmentStyles";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { ErrorNote } from "../workers/parts";
import { useRefreshNotify } from "./NotifyTargets";
import {
  DEFAULT_INTERVAL,
  EMPTY_DRAFT,
  SECURITY_OPTIONS,
  checkDraft,
  notifyFailure,
  secretFrom,
  type DraftErrors,
  type TargetDraft,
  type TargetKind,
} from "./notifyModel";

const KINDS: readonly { kind: TargetKind; label: string }[] = [
  { kind: "webhook", label: "Webhook" },
  { kind: "email", label: "Email" },
];

/** The form that adds a target to one scope. The secret and the password are typed once and never shown again after the add. */
export function AddTargetForm({
  scope,
  canWrite,
}: {
  scope: NotifyScope;
  canWrite: boolean;
}) {
  const [draft, setDraft] = useState<TargetDraft>(EMPTY_DRAFT);
  const [errors, setErrors] = useState<DraftErrors>({});
  const refresh = useRefreshNotify(scope);
  // The add stays pending until the list is read again, so the new target shows only when the daemon has it.
  const add = useMutation({
    mutationFn: (body: NewNotifyTarget) => addNotifyTarget(scope, body),
    onSuccess: async () => {
      await refresh();
      setDraft({ ...EMPTY_DRAFT, kind: draft.kind });
    },
  });
  const set = (patch: Partial<TargetDraft>) => {
    setDraft((current) => ({ ...current, ...patch }));
  };
  const text = (field: keyof DraftErrors) => ({
    value: draft[field],
    error: errors[field],
    disabled: !canWrite || add.isPending,
    onChange: (event: { target: { value: string } }) => {
      set({ [field]: event.target.value });
    },
  });
  const submit = (event: SyntheticEvent) => {
    event.preventDefault();
    const checked = checkDraft(draft);
    if (checked.errors) {
      setErrors(checked.errors);
      return;
    }
    setErrors({});
    add.mutate(checked.body);
  };
  const where = scope.kind === "host" ? "host" : "project";
  return (
    <form
      noValidate
      aria-label={`Add a ${where} target`}
      onSubmit={submit}
      className="flex flex-col gap-14 rounded-m bg-sunken px-16 py-14"
    >
      <div className="flex flex-wrap items-center justify-between gap-10">
        <h3 className="text-bar leading-snug font-semibold text-text">
          Add a target
        </h3>
        <div role="group" aria-label="Kind" className={SEGMENT_LIST}>
          {KINDS.map((option) => (
            <button
              key={option.kind}
              type="button"
              aria-pressed={draft.kind === option.kind}
              disabled={add.isPending}
              onClick={() => {
                setErrors({});
                set({ kind: option.kind });
              }}
              className={cn(
                segmentClasses(draft.kind === option.kind),
                focusRing,
              )}
            >
              {option.label}
            </button>
          ))}
        </div>
      </div>
      {draft.kind === "webhook" ? (
        <div className="grid gap-14 md:grid-cols-2">
          <TextField
            label="URL"
            hint="fold posts a JSON body to it. Lists show only its host."
            autoComplete="off"
            inputMode="url"
            {...text("url")}
          />
          <TextField
            label="Signing secret"
            hint="The key of the x-fold-signature-256 header. Copy it to the receiver now: fold never shows it again."
            autoComplete="off"
            spellCheck={false}
            className="font-mono"
            {...text("secret")}
          />
          <div className="md:col-span-2">
            <Button
              size="sm"
              disabled={!canWrite || add.isPending}
              icon={<KeyRound aria-hidden="true" size={13} />}
              onClick={() => {
                set({
                  secret: secretFrom(
                    crypto.getRandomValues(new Uint8Array(24)),
                  ),
                });
              }}
            >
              Generate a secret
            </Button>
          </div>
        </div>
      ) : (
        <div className="grid gap-14 md:grid-cols-2">
          <TextField
            label="To"
            autoComplete="off"
            inputMode="email"
            {...text("to")}
          />
          <TextField
            label="From"
            autoComplete="off"
            inputMode="email"
            {...text("from")}
          />
          <TextField
            label="SMTP host"
            autoComplete="off"
            {...text("smtpHost")}
          />
          <TextField
            label="SMTP port"
            hint="Empty: the port of the security below."
            inputMode="numeric"
            {...text("smtpPort")}
          />
          <Select
            label="Security"
            value={draft.security}
            disabled={!canWrite || add.isPending}
            onChange={(event) => {
              const chosen = SECURITY_OPTIONS.find(
                (option) => option.value === event.target.value,
              );
              if (chosen) set({ security: chosen.value });
            }}
          >
            {SECURITY_OPTIONS.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </Select>
          <TextField
            label="SMTP user name"
            hint="Empty when the server takes mail without a sign-in."
            autoComplete="off"
            {...text("username")}
          />
          <TextField
            label="SMTP password"
            hint="fold keeps it for the sends and never shows it again."
            type="password"
            autoComplete="new-password"
            {...text("password")}
          />
        </div>
      )}
      <div className="flex flex-wrap items-end justify-between gap-12">
        <TextField
          label="Interval in minutes"
          hint={`The shortest time between two sends; items that come in the meantime go in one digest. Empty: ${String(DEFAULT_INTERVAL)}.`}
          inputMode="numeric"
          {...text("interval")}
        />
        <Button
          type="submit"
          variant="primary"
          disabled={!canWrite}
          pending={add.isPending}
          pendingLabel="Adding…"
          icon={<Plus aria-hidden="true" size={15} />}
        >
          Add target
        </Button>
      </div>
      {add.isError ? <ErrorNote failure={notifyFailure(add.error)} /> : null}
    </form>
  );
}
