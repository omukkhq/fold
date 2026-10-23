import { zodResolver } from "@hookform/resolvers/zod";
import { MessageCircleQuestion } from "lucide-react";
import { Controller, useForm } from "react-hook-form";
import { Button } from "@/components/button/Button";
import type { DiffSelection } from "@/features/diff/selection";
import { MessageComposer } from "@/mentions/MessageComposer";
import { questionSchema, type QuestionValues } from "./compose";
import { askTitle } from "./lineQuestions";

interface AskAuthorBoxProps {
  slug: string;
  selection: DiffSelection;
  /** The version whose lines the question is about. */
  version: number;
  /** Resolves once the daemon has stored the question and the screen read it back; rejects with its error. */
  onSubmit: (body: string) => Promise<void>;
  onCancel: () => void;
  /** The sentence for a failure, from the screen's error mapping. */
  failureText: (error: unknown) => string;
}

/** A short question to the proposal's author about the selected lines. It is a message, never a review comment. */
export function AskAuthorBox({
  slug,
  selection,
  version,
  onSubmit,
  onCancel,
  failureText,
}: AskAuthorBoxProps) {
  const {
    control,
    handleSubmit,
    setError,
    formState: { errors, isSubmitting },
  } = useForm<QuestionValues>({
    resolver: zodResolver(questionSchema),
    defaultValues: { body: "" },
  });
  const title = askTitle(selection, version);

  const submit = handleSubmit(async ({ body }) => {
    try {
      await onSubmit(body.trim());
    } catch (error) {
      setError("root", { message: failureText(error) });
    }
  });

  return (
    <form
      aria-label={title}
      noValidate
      onSubmit={(event) => void submit(event)}
      className="flex flex-col gap-8"
    >
      <p className="flex items-center gap-8 text-code leading-snug font-semibold text-secondary-text">
        <MessageCircleQuestion aria-hidden="true" size={14} />
        {title}
      </p>
      <Controller
        control={control}
        name="body"
        render={({ field }) => (
          <MessageComposer
            slug={slug}
            aria-label="Question"
            autoFocus
            pending={isSubmitting}
            aria-invalid={errors.body ? true : undefined}
            ref={field.ref}
            value={field.value}
            onValueChange={field.onChange}
            onBlur={field.onBlur}
          />
        )}
      />
      {errors.body ? (
        <p className="text-meta font-medium text-danger">
          {errors.body.message}
        </p>
      ) : null}
      {errors.root ? (
        <p role="alert" className="text-meta font-medium text-danger">
          The question was not sent. {errors.root.message}
        </p>
      ) : null}
      <div className="flex flex-wrap items-center gap-8">
        <Button
          type="submit"
          variant="primary"
          pending={isSubmitting}
          pendingLabel="Sending question…"
          icon={<MessageCircleQuestion aria-hidden="true" size={14} />}
        >
          Send question
        </Button>
        <Button variant="ghost" disabled={isSubmitting} onClick={onCancel}>
          Cancel
        </Button>
        <span className="text-meta text-text-muted">
          The author answers in a thread on the Messages screen. The review
          stays as it is.
        </span>
      </div>
    </form>
  );
}
