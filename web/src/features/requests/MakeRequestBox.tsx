import { zodResolver } from "@hookform/resolvers/zod";
import { CircleCheck, FilePlus2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { MessageComposer } from "@/mentions/MessageComposer";
import { projectPath } from "@/routes/paths";
import { newRequestSchema, type NewRequestValues } from "./compose";
import { describeFailure, type Failure } from "./errors";

interface MakeRequestBoxProps {
  slug: string;
  /** The head, such as `Make a request about lines 2–3 · r17`. */
  title: string;
  /** Resolves with the new request's id once the daemon stored it; rejects with its error. */
  onSubmit: (body: string) => Promise<string>;
  onCancel: () => void;
  /** Closes the note that names the new request. */
  onDone: () => void;
}

/** A request about the selected lines. It is an ordinary request that keeps the lines, for the main agent to plan; it is never a review comment or a question to the author. */
export function MakeRequestBox({
  slug,
  title,
  onSubmit,
  onCancel,
  onDone,
}: MakeRequestBoxProps) {
  const [made, setMade] = useState<string | null>(null);
  const [failure, setFailure] = useState<Failure | null>(null);
  const note = useRef<HTMLParagraphElement>(null);
  // The form that held focus is gone once the request is made, so focus moves to the note that names it.
  useEffect(() => {
    if (made !== null) note.current?.focus();
  }, [made]);
  const {
    control,
    handleSubmit,
    formState: { errors, isSubmitting },
  } = useForm<NewRequestValues>({
    resolver: zodResolver(newRequestSchema),
    defaultValues: { body: "" },
  });

  const submit = handleSubmit(async ({ body }) => {
    setFailure(null);
    try {
      setMade(await onSubmit(body.trim()));
    } catch (error) {
      setFailure(describeFailure(error));
    }
  });

  if (made !== null) {
    return (
      <div className="flex flex-wrap items-center gap-8">
        <p
          ref={note}
          role="status"
          tabIndex={-1}
          className={cn(
            "flex items-center gap-8 rounded-s text-14 text-text",
            focusRing,
          )}
        >
          <CircleCheck aria-hidden="true" size={14} className="text-ok" />
          <span>
            fold made{" "}
            <HrefLink
              href={projectPath(slug, "requests", made)}
              className={cn(
                "rounded-s font-medium text-accent tabular-nums underline",
                focusRing,
              )}
            >
              {made}
            </HrefLink>{" "}
            with these lines. The main agent plans it.
          </span>
        </p>
        <Button variant="ghost" size="sm" onClick={onDone}>
          Done
        </Button>
      </div>
    );
  }

  return (
    <form
      aria-label={title}
      noValidate
      onSubmit={(event) => void submit(event)}
      className="flex flex-col gap-8"
    >
      <p className="flex items-center gap-8 text-13 leading-tight font-semibold text-text">
        <FilePlus2 aria-hidden="true" size={14} className="text-accent" />
        {title}
      </p>
      <Controller
        control={control}
        name="body"
        render={({ field }) => (
          <MessageComposer
            slug={slug}
            aria-label="Request"
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
        <p className="text-13 font-medium text-bad">{errors.body.message}</p>
      ) : null}
      {failure ? (
        <p role="alert" className="text-13 font-medium text-bad">
          The request was not made. {failure.text}
          {failure.code ? (
            <span className="font-mono font-regular">
              {" "}
              code: {failure.code}
            </span>
          ) : null}
        </p>
      ) : null}
      <div className="flex flex-wrap items-center gap-8">
        <Button
          type="submit"
          variant="primary"
          pending={isSubmitting}
          pendingLabel="Making the request…"
          icon={<FilePlus2 aria-hidden="true" size={14} />}
        >
          Make the request
        </Button>
        <Button variant="ghost" disabled={isSubmitting} onClick={onCancel}>
          Cancel
        </Button>
        <span className="text-13 text-text-2">
          The request keeps these lines. The main agent plans it on the Requests
          screen.
        </span>
      </div>
    </form>
  );
}
