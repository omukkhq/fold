import {
  type UseMutationResult,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { dismissSharedChanges } from "@/api/repository";
import { keys } from "@/api/keys";
import { sharedChangesQuery } from "@/api/repositoryQueries";
import { Button } from "@/components/button/Button";
import { Mono } from "@/components/mono/Mono";
import { Dialog } from "@/components/overlay/Dialog";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { WriteError } from "@/features/task/parts";
import { dismissLabel, settleFailure } from "./agentChanges";
import { folderFailure, sharedFileFacts } from "./flaggedFiles";

type Dismiss = UseMutationResult<unknown, Error, void>;

/** The write that ends every Needs you item of changed shared files; it stays pending until Needs you is read again. */
export function useDismissShared(slug: string, onDone?: () => void): Dismiss {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      const done = await dismissSharedChanges(slug);
      await queryClient.invalidateQueries({ queryKey: keys.needsYou(slug) });
      return done;
    },
    onSuccess: onDone,
  });
}

function DismissError({ dismiss }: { dismiss: Dismiss }) {
  const failed = dismiss.error ? settleFailure(dismiss.error) : null;
  return (
    <WriteError
      error={failed === null ? null : dismiss.error}
      describe={() => (failed === null ? null : { field: null, text: failed })}
    />
  );
}

function DismissButton({
  dismiss,
  count,
  size = "sm",
}: {
  dismiss: Dismiss;
  count: number;
  size?: "sm" | "md";
}) {
  return (
    <Button
      size={size}
      variant="ghost"
      pending={dismiss.isPending}
      pendingLabel="Dismissing…"
      onClick={() => {
        dismiss.mutate();
      }}
    >
      {dismissLabel(count)}
    </Button>
  );
}

/** Dismiss under the Needs you line of a changed shared file, which ends every such item. */
export function SharedChangeDismiss({
  slug,
  count,
}: {
  slug: string;
  count: number;
}) {
  const dismiss = useDismissShared(slug);
  return (
    <div className="flex flex-col gap-4">
      <div>
        <DismissButton dismiss={dismiss} count={count} />
      </div>
      <DismissError dismiss={dismiss} />
    </div>
  );
}

function SharedFileList({ slug, now }: { slug: string; now: number }) {
  const files = useQuery(sharedChangesQuery(slug));
  if (files.isError) {
    return (
      <ErrorBanner
        className="rounded-m px-12"
        title="The changed files could not load."
        message={folderFailure(files.error)}
        onRetry={() => void files.refetch()}
      />
    );
  }
  if (!files.data) return <Skeleton />;
  return (
    <ul className="flex min-w-0 flex-col gap-6">
      {files.data.map((file) => (
        <li key={file.path} className="flex min-w-0 flex-col">
          <Mono className="min-w-0 wrap-anywhere text-text">{file.path}</Mono>
          <span className="text-13 text-text-2">
            {sharedFileFacts(file, now)}
          </span>
        </li>
      ))}
    </ul>
  );
}

/** The changed shared files with their size and change time now: ignored files have no history, so these stand for a diff. Dismiss ends every such item. */
export function SharedFilesDialog({
  slug,
  count,
  now,
  open,
  onClose,
}: {
  slug: string;
  count: number;
  now: number;
  open: boolean;
  onClose: () => void;
}) {
  const dismiss = useDismissShared(slug, onClose);
  const close = () => {
    if (dismiss.isPending) return;
    dismiss.reset();
    onClose();
  };
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) close();
      }}
      title="Shared files that changed while an agent worked"
      description="Shared files are ignored, so fold keeps no history of them: check each file in the project folder by its size and change time, then dismiss."
      wide
      footer={
        <div className="flex flex-wrap justify-end gap-8">
          <Button variant="ghost" disabled={dismiss.isPending} onClick={close}>
            Close
          </Button>
          <DismissButton dismiss={dismiss} count={count} size="md" />
        </div>
      }
    >
      <DismissError dismiss={dismiss} />
      {open ? <SharedFileList slug={slug} now={now} /> : null}
    </Dialog>
  );
}
