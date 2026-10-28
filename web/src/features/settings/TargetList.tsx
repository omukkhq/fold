import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  CircleCheck,
  KeyRound,
  ListOrdered,
  Pencil,
  RotateCw,
  Send,
  Trash2,
} from "lucide-react";
import { useState } from "react";
import { keys } from "@/api/keys";
import {
  removeNotifyTarget,
  rotateNotifyTarget,
  sendNotifyAgain,
  testNotifyTarget,
  type NotifyScope,
  type NotifySend,
  type NotifyTarget,
  type NotifyTargetAdded,
} from "@/api/notify";
import { notifySendsQuery } from "@/api/queries";
import { Button } from "@/components/button/Button";
import { Mono } from "@/components/mono/Mono";
import { Skeleton } from "@/components/states/Skeleton";
import { cn } from "@/lib/cn";
import { useNow } from "@/live/useNow";
import { ErrorNote } from "../workers/parts";
import { notifyFailure, sendView, targetView, type Tone } from "./notifyModel";
import { TargetForm } from "./TargetForm";

const TONES: Record<Tone, string> = {
  ok: "bg-success-soft text-success-text",
  bad: "bg-danger-soft text-danger",
  muted: "bg-sunken text-text-muted",
};

const ANSWER_TONES: Record<Tone, string> = {
  ok: "text-success-text",
  bad: "text-danger",
  muted: "text-text-muted",
};

/** Refreshes every read of the notice settings: a host target shows on every project's page too. */
export function useRefreshNotify() {
  const queryClient = useQueryClient();
  return async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: keys.hostNotify() }),
      queryClient.invalidateQueries({
        predicate: (query) => query.queryKey[1] === "notify",
      }),
    ]);
  };
}

function SendRow({
  slug,
  send,
  now,
  canWrite,
}: {
  slug: string;
  send: NotifySend;
  now: number;
  canWrite: boolean;
}) {
  const refresh = useRefreshNotify();
  const again = useMutation({
    mutationFn: () => sendNotifyAgain(slug, send.id),
    onSuccess: refresh,
  });
  const view = sendView(send, now);
  return (
    <li className="grid grid-cols-[auto_minmax(0,1fr)_auto_auto] items-baseline gap-10 border-t border-border-subtle py-6 text-meta">
      <span className="text-text-faint tabular-nums">{view.time}</span>
      <span className="truncate text-text">{view.summary}</span>
      <Mono className={cn("text-meta", ANSWER_TONES[view.tone])}>
        {view.answer} · {view.tries}
      </Mono>
      {view.canSendAgain ? (
        <Button
          size="sm"
          variant="ghost"
          disabled={!canWrite}
          pending={again.isPending}
          pendingLabel="Sending…"
          icon={<RotateCw aria-hidden="true" size={13} />}
          aria-label={`Send ${send.id} again`}
          onClick={() => {
            again.mutate();
          }}
        >
          Send again
        </Button>
      ) : (
        <span />
      )}
      {again.isError ? (
        <ErrorNote
          className="col-span-4"
          failure={notifyFailure(again.error)}
        />
      ) : null}
    </li>
  );
}

function SendLog({
  slug,
  target,
  canWrite,
}: {
  slug: string;
  target: NotifyTarget;
  canWrite: boolean;
}) {
  const sends = useQuery(notifySendsQuery(slug, target.id));
  const now = useNow(true, 30_000);
  if (sends.isError) {
    return <ErrorNote failure={notifyFailure(sends.error)} />;
  }
  if (sends.data === undefined) {
    return <Skeleton />;
  }
  return (
    <section
      aria-label={`Recent sends of ${target.name}`}
      className="flex flex-col"
    >
      {sends.data.length === 0 ? (
        <p className="text-meta text-text-muted">No sends yet.</p>
      ) : (
        <ul className="flex flex-col">
          {sends.data.map((send) => (
            <SendRow
              key={send.id}
              slug={slug}
              send={send}
              now={now}
              canWrite={canWrite}
            />
          ))}
        </ul>
      )}
      <p className="pt-6 text-meta text-text-muted">
        fold keeps the last 50 sends of each target.
      </p>
    </section>
  );
}

function TargetRow({
  slug,
  target,
  canWrite,
  onSecret,
}: {
  slug: string;
  target: NotifyTarget;
  canWrite: boolean;
  onSecret: (added: NotifyTargetAdded) => void;
}) {
  const scope: NotifyScope = target.host
    ? { kind: "host" }
    : { kind: "project", slug };
  const refresh = useRefreshNotify();
  const [open, setOpen] = useState<"edit" | "log" | null>(null);
  const now = useNow(true, 30_000);
  const test = useMutation({
    mutationFn: () => testNotifyTarget(slug, target.id),
    onSettled: refresh,
  });
  const remove = useMutation({
    mutationFn: () => removeNotifyTarget(scope, target.id),
    onSuccess: refresh,
  });
  const rotate = useMutation({
    mutationFn: () => rotateNotifyTarget(scope, target.id),
    gcTime: 0,
    onSuccess: async (added) => {
      onSecret(added);
      await refresh();
      rotate.reset();
    },
  });
  const view = targetView(target, now);
  return (
    <li className="flex flex-col gap-10 rounded-m px-14 py-12 inset-ring inset-ring-border-subtle">
      <div className="flex flex-wrap items-start justify-between gap-12">
        <div className="flex min-w-0 flex-col gap-4">
          <p className="flex flex-wrap items-center gap-8 text-body leading-snug font-semibold text-text">
            {view.title}
            <span
              className={cn(
                "rounded-s px-8 py-2 text-meta font-medium",
                TONES[view.state.tone],
              )}
            >
              {view.state.label}
            </span>
            {view.health ? (
              <span className="text-meta font-normal text-text-muted">
                {view.health}
              </span>
            ) : null}
          </p>
          <p className="text-meta break-all text-text-muted">
            <Mono className="text-meta text-text-muted">{target.id}</Mono>
            {" · "}
            {view.detail}
          </p>
          <p className="text-meta text-text-faint">{view.scope}</p>
        </div>
        <div className="flex shrink-0 flex-wrap items-center gap-8">
          <Button
            size="sm"
            disabled={!canWrite}
            pending={test.isPending}
            pendingLabel="Sending…"
            icon={<Send aria-hidden="true" size={13} />}
            aria-label={`Send a test through ${target.name}`}
            onClick={() => {
              test.mutate();
            }}
          >
            Send a test
          </Button>
          <Button
            size="sm"
            variant="ghost"
            aria-pressed={open === "log"}
            icon={<ListOrdered aria-hidden="true" size={13} />}
            onClick={() => {
              setOpen(open === "log" ? null : "log");
            }}
          >
            Sends
          </Button>
          <Button
            size="sm"
            variant="ghost"
            disabled={!canWrite}
            aria-pressed={open === "edit"}
            icon={<Pencil aria-hidden="true" size={13} />}
            aria-label={`Edit ${target.name}`}
            onClick={() => {
              setOpen(open === "edit" ? null : "edit");
            }}
          >
            Edit
          </Button>
          {target.kind === "webhook" ? (
            <Button
              size="sm"
              variant="ghost"
              disabled={!canWrite}
              pending={rotate.isPending}
              pendingLabel="Rotating…"
              icon={<KeyRound aria-hidden="true" size={13} />}
              aria-label={`New signing secret for ${target.name}`}
              onClick={() => {
                rotate.mutate();
              }}
            >
              New secret
            </Button>
          ) : null}
          <Button
            size="sm"
            variant="ghost"
            className="text-danger hover:bg-danger-soft"
            disabled={!canWrite}
            pending={remove.isPending}
            pendingLabel="Removing…"
            icon={<Trash2 aria-hidden="true" size={13} />}
            aria-label={`Remove ${target.name}`}
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
          className="flex items-center gap-6 text-bar text-success-text"
        >
          <CircleCheck aria-hidden="true" size={14} />
          The receiver took the test.
        </p>
      ) : null}
      {test.isError ? <ErrorNote failure={notifyFailure(test.error)} /> : null}
      {remove.isError ? (
        <ErrorNote failure={notifyFailure(remove.error)} />
      ) : null}
      {rotate.isError ? (
        <ErrorNote failure={notifyFailure(rotate.error)} />
      ) : null}
      {open === "edit" ? (
        <TargetForm
          scope={scope}
          editing={target}
          canWrite={canWrite}
          onSaved={async () => {
            await refresh();
            setOpen(null);
          }}
          onCancel={() => {
            setOpen(null);
          }}
        />
      ) : null}
      {open === "log" ? (
        <SendLog slug={slug} target={target} canWrite={canWrite} />
      ) : null}
    </li>
  );
}

/** The targets that get the project's notices: its own, then the host's, each with its test, send log, edit and removal. */
export function TargetList({
  slug,
  targets,
  canWrite,
  onSecret,
}: {
  slug: string;
  targets: readonly NotifyTarget[];
  canWrite: boolean;
  onSecret: (added: NotifyTargetAdded) => void;
}) {
  if (targets.length === 0) {
    return (
      <p className="text-bar text-text-muted">No targets yet. Add one below.</p>
    );
  }
  return (
    <ul aria-label="Targets" className="flex flex-col gap-10">
      {targets.map((target) => (
        <TargetRow
          key={target.id}
          slug={slug}
          target={target}
          canWrite={canWrite}
          onSecret={onSecret}
        />
      ))}
    </ul>
  );
}
