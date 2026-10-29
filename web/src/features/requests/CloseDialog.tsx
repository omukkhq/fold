import { zodResolver } from "@hookform/resolvers/zod";
import { X } from "lucide-react";
import { Controller, useForm } from "react-hook-form";
import { Button } from "@/components/button/Button";
import { Dialog } from "@/components/overlay/Dialog";
import { MentionTextarea } from "@/mentions/MentionTextarea";
import { closeSchema, type CloseValues } from "./compose";
import type { Failure } from "./errors";

interface CloseDialogProps {
  slug: string;
  id: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  failure: Failure | null;
  /** Resolves once the daemon closed the request; rejects with its error. */
  onCloseRequest: (reason: string) => Promise<void>;
}

export function CloseDialog({
  slug,
  id,
  open,
  onOpenChange,
  failure,
  onCloseRequest,
}: CloseDialogProps) {
  const {
    control,
    handleSubmit,
    reset,
    formState: { errors, isSubmitting },
  } = useForm<CloseValues>({
    resolver: zodResolver(closeSchema),
    defaultValues: { reason: "" },
  });
  const submit = handleSubmit(async ({ reason }) => {
    try {
      await onCloseRequest(reason.trim());
      reset();
      onOpenChange(false);
    } catch {
      // The dialog shows the failure it was handed.
    }
  });
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!isSubmitting) onOpenChange(next);
      }}
      title={`Close ${id}`}
      description="A closed request only reads. Say why, so the orchestrator and the others know."
    >
      <form
        noValidate
        onSubmit={(event) => void submit(event)}
        className="flex w-thread-list max-w-full flex-col gap-10"
      >
        <label
          htmlFor="close-reason"
          className="text-14 leading-tight font-semibold text-text"
        >
          Reason
        </label>
        <Controller
          control={control}
          name="reason"
          render={({ field }) => (
            <MentionTextarea
              id="close-reason"
              slug={slug}
              rows={3}
              ref={field.ref}
              value={field.value}
              onValueChange={field.onChange}
              onBlur={field.onBlur}
              readOnly={isSubmitting}
              aria-invalid={errors.reason ? true : undefined}
              placeholder="Done another way in #T-9."
            />
          )}
        />
        {errors.reason ? (
          <p className="text-13 font-medium text-bad">
            {errors.reason.message}
          </p>
        ) : null}
        {failure ? (
          <p role="alert" className="text-13 font-medium text-bad">
            {failure.text}
            {failure.code ? (
              <span className="font-mono font-regular">
                {" "}
                code: {failure.code}
              </span>
            ) : null}
          </p>
        ) : null}
        <div className="flex justify-end gap-8">
          <Button
            variant="ghost"
            disabled={isSubmitting}
            onClick={() => {
              onOpenChange(false);
            }}
          >
            Cancel
          </Button>
          <Button
            type="submit"
            variant="danger"
            pending={isSubmitting}
            pendingLabel="Closing request…"
            icon={<X aria-hidden="true" size={15} />}
          >
            Close request
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
