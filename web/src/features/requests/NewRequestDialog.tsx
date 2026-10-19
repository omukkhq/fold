import { zodResolver } from "@hookform/resolvers/zod";
import * as RadixDialog from "@radix-ui/react-dialog";
import { useQueryClient } from "@tanstack/react-query";
import { FileText, ImageIcon, Send, Upload, UserRound, X } from "lucide-react";
import { useId, useRef, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { keys } from "@/api/keys";
import { createRequest } from "@/api/requests";
import { Button, IconButton } from "@/components/button/Button";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { MentionTextarea } from "@/mentions/MentionTextarea";
import {
  MAX_FILES,
  addFiles,
  blockingProblem,
  fileProblem,
  refusedText,
} from "./attachments";
import { newRequestSchema, type NewRequestValues } from "./compose";
import { describeFailure, type Failure } from "./errors";
import { FailurePanel } from "@/components/states/FailurePanel";
import { formatSize } from "./present";

interface NewRequestDialogProps {
  slug: string;
  human: string | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Called with the new request's id once the daemon has made it. */
  onCreated?: (request: string) => void;
}

function FileRow({
  file,
  disabled,
  onRemove,
}: {
  file: File;
  disabled: boolean;
  onRemove: () => void;
}) {
  const problem = fileProblem(file);
  const Icon = file.type.startsWith("image/") ? ImageIcon : FileText;
  return (
    <li
      className={cn(
        "flex items-center gap-10 rounded-m bg-surface px-12 py-8 inset-ring",
        problem ? "inset-ring-danger" : "inset-ring-border",
      )}
    >
      <Icon aria-hidden="true" size={15} className="shrink-0 text-text-muted" />
      <span className="min-w-0 flex-1 truncate text-body text-text">
        {file.name}
      </span>
      <span
        className={cn(
          "font-mono text-meta",
          problem ? "text-danger" : "text-text-muted",
        )}
      >
        {formatSize(file.size)}
      </span>
      <IconButton
        aria-label={`Remove ${file.name}`}
        size="sm"
        disabled={disabled}
        onClick={onRemove}
      >
        <X aria-hidden="true" size={13} />
      </IconButton>
    </li>
  );
}

export function NewRequestDialog({
  slug,
  human,
  open,
  onOpenChange,
  onCreated,
}: NewRequestDialogProps) {
  const queryClient = useQueryClient();
  const [files, setFiles] = useState<File[]>([]);
  const [refused, setRefused] = useState(0);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [dragging, setDragging] = useState(false);
  const picker = useRef<HTMLInputElement>(null);
  const hintId = useId();
  const {
    control,
    handleSubmit,
    reset,
    formState: { errors, isSubmitting },
  } = useForm<NewRequestValues>({
    resolver: zodResolver(newRequestSchema),
    defaultValues: { body: "" },
  });

  const take = (added: readonly File[]) => {
    const next = addFiles(files, added);
    setFiles(next.files);
    setRefused(next.refused);
  };

  const close = () => {
    reset();
    setFiles([]);
    setRefused(0);
    setFailure(null);
    onOpenChange(false);
  };

  const blocking = blockingProblem(files);
  const submit = handleSubmit(async ({ body }) => {
    if (blocking) return;
    setFailure(null);
    try {
      const detail = await createRequest(slug, body, files);
      queryClient.setQueryData(keys.request(slug, detail.request.id), detail);
      await queryClient.invalidateQueries({ queryKey: keys.requests(slug) });
      close();
      onCreated?.(detail.request.id);
    } catch (error) {
      setFailure(describeFailure(error));
    }
  });

  return (
    <RadixDialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next && !isSubmitting) close();
      }}
    >
      <RadixDialog.Portal>
        {/* z-10 matches the task dialog, so the request dialog opened from New task draws over it. */}
        <RadixDialog.Overlay className="fixed inset-0 z-10 bg-scrim" />
        <RadixDialog.Content className="fixed top-1/2 z-10 left-1/2 flex max-h-full w-request-dialog max-w-full -translate-x-1/2 -translate-y-1/2 flex-col overflow-y-auto rounded-l bg-raised shadow-dialog inset-ring inset-ring-border outline-none">
          <div className="flex items-center justify-between border-b border-border-subtle px-24 py-16">
            <RadixDialog.Title className="text-section leading-snug font-semibold text-text">
              New request
            </RadixDialog.Title>
            <RadixDialog.Close asChild>
              <IconButton aria-label="Close" size="sm" disabled={isSubmitting}>
                <X aria-hidden="true" size={13} />
              </IconButton>
            </RadixDialog.Close>
          </div>
          <form
            noValidate
            onSubmit={(event) => void submit(event)}
            className="flex flex-col"
          >
            <div className="flex flex-col gap-16 px-24 pt-20 pb-20">
              <RadixDialog.Description className="text-bar text-text-muted">
                An orchestrator reads your message, writes the title, sets the
                kind and drafts the cards for you to approve.
              </RadixDialog.Description>
              {failure ? (
                <FailurePanel
                  title="fold could not create the request"
                  text={failure.text}
                  code={failure.code}
                />
              ) : null}
              <div className="flex flex-col gap-6">
                <label
                  htmlFor={`${hintId}-body`}
                  className="text-bar leading-snug font-semibold text-text"
                >
                  What do you need?
                </label>
                <Controller
                  control={control}
                  name="body"
                  render={({ field }) => (
                    <MentionTextarea
                      id={`${hintId}-body`}
                      slug={slug}
                      rows={7}
                      ref={field.ref}
                      value={field.value}
                      onValueChange={field.onChange}
                      onBlur={field.onBlur}
                      readOnly={isSubmitting}
                      aria-describedby={hintId}
                      aria-invalid={errors.body ? true : undefined}
                      placeholder="Product wants the weekly report out of fold. The report page from #T-9 already has the data; see @web/src/report.tsx."
                      className="resize-y"
                    />
                  )}
                />
                <p id={hintId} className="text-meta text-text-muted">
                  Markdown is allowed · # for tasks, proposals and requests · @
                  for files
                </p>
                {errors.body ? (
                  <p className="text-meta font-medium text-danger">
                    {errors.body.message}
                  </p>
                ) : null}
              </div>
              <div className="flex flex-col gap-8">
                <p className="text-bar leading-snug font-semibold text-text">
                  Attachments{" "}
                  <span className="font-normal text-text-muted">optional</span>
                </p>
                <button
                  type="button"
                  disabled={isSubmitting || files.length >= MAX_FILES}
                  onClick={() => picker.current?.click()}
                  onDragOver={(event) => {
                    event.preventDefault();
                    setDragging(true);
                  }}
                  onDragLeave={() => {
                    setDragging(false);
                  }}
                  onDrop={(event) => {
                    event.preventDefault();
                    setDragging(false);
                    if (!isSubmitting) take([...event.dataTransfer.files]);
                  }}
                  className={cn(
                    "flex items-center justify-center gap-8 rounded-m bg-sunken px-12 py-12 text-bar text-text-muted inset-ring inset-ring-border disabled:cursor-not-allowed disabled:opacity-60",
                    dragging && "inset-ring-primary",
                    focusRing,
                  )}
                >
                  <Upload aria-hidden="true" size={15} />
                  Drop files or browse · up to 10 files, 10 MiB each
                </button>
                <input
                  ref={picker}
                  type="file"
                  multiple
                  hidden
                  aria-label="Attach files"
                  onChange={(event) => {
                    take([...(event.target.files ?? [])]);
                    event.target.value = "";
                  }}
                />
                {files.length > 0 ? (
                  <ul
                    aria-label="Attached files"
                    className="flex flex-col gap-8"
                  >
                    {files.map((file, index) => (
                      <FileRow
                        key={`${file.name}:${String(index)}`}
                        file={file}
                        disabled={isSubmitting}
                        onRemove={() => {
                          setFiles(files.filter((_, i) => i !== index));
                          setRefused(0);
                        }}
                      />
                    ))}
                  </ul>
                ) : null}
                {blocking || refused > 0 ? (
                  <p role="alert" className="text-meta font-medium text-danger">
                    {blocking ?? refusedText(refused)}
                  </p>
                ) : null}
              </div>
            </div>
            <div className="flex items-center gap-12 border-t border-border-subtle px-24 py-14">
              <p className="flex min-w-0 flex-1 items-center gap-6 text-bar text-text-muted">
                <UserRound aria-hidden="true" size={15} className="shrink-0" />
                {human === null
                  ? "Tell fold your name first, then create the request."
                  : isSubmitting
                    ? `Creating as ${human}. The request appears in the list when fold confirms.`
                    : `Requested as ${human}`}
              </p>
              <Button disabled={isSubmitting} onClick={close}>
                Cancel
              </Button>
              <Button
                type="submit"
                variant="primary"
                pending={isSubmitting}
                pendingLabel="Creating request…"
                disabled={blocking !== null}
                icon={<Send aria-hidden="true" size={15} />}
              >
                Create request
              </Button>
            </div>
          </form>
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}
