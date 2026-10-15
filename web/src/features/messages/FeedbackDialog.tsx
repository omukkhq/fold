import { zodResolver } from "@hookform/resolvers/zod";
import { Controller, useForm } from "react-hook-form";
import { Button } from "@/components/button/Button";
import { TextField } from "@/components/form/controls";
import { Dialog } from "@/components/overlay/Dialog";
import { MentionField } from "@/mentions/MentionField";
import { failureText } from "@/format/failure";
import { feedbackSchema, type FeedbackValues } from "./compose";

interface FeedbackDialogProps {
  slug: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Resolves once the daemon has taken the message; rejects with its error. */
  onSend: (task: string, body: string) => Promise<void>;
}

export function FeedbackDialog({
  slug,
  open,
  onOpenChange,
  onSend,
}: FeedbackDialogProps) {
  const {
    register,
    control,
    handleSubmit,
    reset,
    setError,
    formState: { errors, isSubmitting },
  } = useForm<FeedbackValues>({
    resolver: zodResolver(feedbackSchema),
    defaultValues: { task: "", body: "" },
  });

  const submit = handleSubmit(async (values) => {
    try {
      await onSend(values.task.trim(), values.body.trim());
      reset();
      onOpenChange(false);
    } catch (error) {
      setError("root", { message: failureText(error) });
    }
  });

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!isSubmitting) {
          onOpenChange(next);
        }
      }}
      title="Send feedback"
      description="The message reaches the agent that works on the task. It changes nothing on the task."
    >
      <form
        onSubmit={(event) => void submit(event)}
        className="flex flex-col gap-12"
        noValidate
      >
        <TextField
          label="Task"
          placeholder="T-12"
          error={errors.task?.message}
          {...register("task")}
        />
        <Controller
          control={control}
          name="body"
          render={({ field }) => (
            <MentionField
              slug={slug}
              label="Message"
              error={errors.body?.message}
              ref={field.ref}
              value={field.value}
              onValueChange={field.onChange}
              onBlur={field.onBlur}
            />
          )}
        />
        {errors.root ? (
          <p role="alert" className="text-bar font-medium text-danger">
            The message was not sent. {errors.root.message}
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
          <Button type="submit" variant="primary" pending={isSubmitting}>
            Send feedback
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
