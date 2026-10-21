import { useMutation, useQueryClient } from "@tanstack/react-query";
import { CircleCheck, Mail, Send, Trash2, Webhook } from "lucide-react";
import { keys } from "@/api/keys";
import {
  removeNotifyTarget,
  testNotifyTarget,
  type NotifyScope,
  type NotifyTarget,
} from "@/api/notify";
import { Button } from "@/components/button/Button";
import { Mono } from "@/components/mono/Mono";
import { ErrorNote } from "../workers/parts";
import { notifyFailure, targetView } from "./notifyModel";

/** Refreshes both settings reads: a host target shows on every project's settings too. */
export function useRefreshNotify(scope: NotifyScope) {
  const queryClient = useQueryClient();
  return () =>
    Promise.all([
      queryClient.invalidateQueries({ queryKey: keys.hostNotify() }),
      scope.kind === "project"
        ? queryClient.invalidateQueries({ queryKey: keys.notify(scope.slug) })
        : queryClient.invalidateQueries({
            predicate: (query) => query.queryKey[1] === "notify",
          }),
    ]);
}

function TargetRow({
  scope,
  target,
  canWrite,
}: {
  scope: NotifyScope;
  target: NotifyTarget;
  canWrite: boolean;
}) {
  const refresh = useRefreshNotify(scope);
  const test = useMutation({
    mutationFn: () => testNotifyTarget(scope, target.id),
  });
  // The removal stays pending until the list is read again, so the row never leaves before the daemon's settings do.
  const remove = useMutation({
    mutationFn: () => removeNotifyTarget(scope, target.id),
    onSuccess: refresh,
  });
  const view = targetView(target);
  const Icon = view.kind === "webhook" ? Webhook : Mail;
  return (
    <li className="flex flex-col gap-10 border-b border-border-subtle py-12 last:border-b-0">
      <div className="flex flex-wrap items-start justify-between gap-12">
        <div className="flex min-w-0 items-start gap-10">
          <Icon
            aria-hidden="true"
            size={16}
            className="mt-2 shrink-0 text-text-muted"
          />
          <div className="flex min-w-0 flex-col gap-2">
            <p className="text-body leading-snug font-medium break-all text-text">
              {view.title}
            </p>
            <p className="text-meta text-text-muted">
              <Mono className="text-meta text-text-muted">{target.id}</Mono>
              {" · "}
              {view.detail}
              {" · "}
              {view.interval}
            </p>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-8">
          <Button
            size="sm"
            disabled={!canWrite}
            pending={test.isPending}
            pendingLabel="Sending…"
            icon={<Send aria-hidden="true" size={13} />}
            aria-label={`Send a test notice through ${target.id}`}
            onClick={() => {
              test.mutate();
            }}
          >
            Send test
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="text-danger hover:bg-danger-soft"
            disabled={!canWrite}
            pending={remove.isPending}
            pendingLabel="Removing…"
            icon={<Trash2 aria-hidden="true" size={13} />}
            aria-label={`Remove ${target.id}`}
            onClick={() => {
              remove.mutate();
            }}
          >
            Remove
          </Button>
        </div>
      </div>
      {test.isSuccess ? (
        <p
          role="status"
          className="flex items-center gap-6 text-bar text-success"
        >
          <CircleCheck aria-hidden="true" size={14} />
          The receiver took the test notice.
        </p>
      ) : null}
      {test.isError ? <ErrorNote failure={notifyFailure(test.error)} /> : null}
      {remove.isError ? (
        <ErrorNote failure={notifyFailure(remove.error)} />
      ) : null}
    </li>
  );
}

/** The targets of one scope, oldest first, each with its test send and removal. */
export function NotifyTargets({
  scope,
  targets,
  canWrite,
  label,
}: {
  scope: NotifyScope;
  targets: readonly NotifyTarget[];
  canWrite: boolean;
  label: string;
}) {
  if (targets.length === 0) {
    return null;
  }
  return (
    <ul aria-label={label} className="flex flex-col">
      {targets.map((target) => (
        <TargetRow
          key={target.id}
          scope={scope}
          target={target}
          canWrite={canWrite}
        />
      ))}
    </ul>
  );
}
