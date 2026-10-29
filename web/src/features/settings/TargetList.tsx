import * as RadixDialog from "@radix-ui/react-dialog";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  CircleCheck,
  KeyRound,
  ListOrdered,
  Pencil,
  RotateCw,
  Send,
  Trash2,
  X,
} from "lucide-react";
import { useEffect, useState } from "react";
import {
  editNotifyTarget,
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
import { Button, IconButton } from "@/components/button/Button";
import { Checkbox } from "@/components/form/controls";
import { Chip } from "@/components/badge/Chip";
import type { ChipTone } from "@/components/badge/chips";
import { Id } from "@/components/id/Id";
import { Skeleton } from "@/components/states/Skeleton";
import { TABLE_ROW } from "@/components/table/tableStyles";
import { IdText } from "@/components/text/IdText";
import { cn } from "@/lib/cn";
import { useNow } from "@/live/useNow";
import { ErrorNote } from "../workers/parts";
import {
  TEST_NOTE_MS,
  enabledHint,
  notifyFailure,
  refreshedByNotifyWrite,
  sendView,
  targetView,
  type Tone,
} from "./notifyModel";
import { TargetForm } from "./TargetForm";

const CHIP_TONES: Record<Tone, ChipTone> = {
  ok: "ok",
  bad: "bad",
  muted: "neutral",
};

const ANSWER_TONES: Record<Tone, string> = {
  ok: "text-ok",
  bad: "text-bad",
  muted: "text-text-2",
};

/** Reads the notice settings, the send logs and Needs you again after a person's own write. */
export function useRefreshNotify() {
  const queryClient = useQueryClient();
  return async () => {
    await queryClient.invalidateQueries({
      predicate: (query) => refreshedByNotifyWrite(query.queryKey),
    });
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
    <li
      className={cn(
        "grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-10 gap-y-4 rounded-s px-10 py-8 text-14 sm:grid-cols-[auto_minmax(0,1fr)_auto_auto]",
        TABLE_ROW,
      )}
    >
      <span className="text-13 whitespace-nowrap text-text-2 tabular-nums">
        {view.time}
      </span>
      <span className="min-w-0 break-words text-text">
        <IdText text={view.summary} />
      </span>
      <span className="col-start-2 flex flex-wrap items-baseline gap-x-10 gap-y-4 sm:contents">
        <span
          className={cn(
            "text-13 whitespace-nowrap tabular-nums",
            ANSWER_TONES[view.tone],
          )}
        >
          {view.answer} · {view.tries}
        </span>
        {view.canSendAgain ? (
          <Button
            size="sm"
            variant="ghost"
            disabled={!canWrite}
            pending={again.isPending}
            pendingLabel="Sending…"
            icon={<RotateCw aria-hidden="true" size={14} />}
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
      </span>
      {again.isError ? (
        <ErrorNote
          className="col-span-full"
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
      className="flex flex-col gap-6"
    >
      {sends.data.length === 0 ? (
        <p className="text-14 text-text-2">No sends yet.</p>
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
      <p className="text-13 text-text-2">
        fold keeps the last 50 sends of each target.
      </p>
    </section>
  );
}

/** The removal of a target, which asks first: the URL of a Slack or Discord target is its secret, and a generic target's signing secret goes with it, so neither comes back. The confirm stays pending until the daemon answers. */
function RemoveTarget({
  scope,
  target,
  canWrite,
}: {
  scope: NotifyScope;
  target: NotifyTarget;
  canWrite: boolean;
}) {
  const refresh = useRefreshNotify();
  const [open, setOpen] = useState(false);
  const remove = useMutation({
    mutationFn: () => removeNotifyTarget(scope, target.id),
    onSuccess: async () => {
      await refresh();
      setOpen(false);
    },
  });
  const close = () => {
    if (remove.isPending) return;
    remove.reset();
    setOpen(false);
  };
  return (
    <RadixDialog.Root
      open={open}
      onOpenChange={(next) => {
        if (next) setOpen(true);
        else close();
      }}
    >
      <RadixDialog.Trigger asChild>
        <Button
          size="sm"
          variant="ghost"
          className="text-bad hover:bg-bad-soft"
          disabled={!canWrite}
          icon={<Trash2 aria-hidden="true" size={14} />}
          aria-label={`Remove ${target.name}`}
        >
          Remove
        </Button>
      </RadixDialog.Trigger>
      <RadixDialog.Portal>
        <RadixDialog.Overlay className="fixed inset-0 z-dialog bg-scrim" />
        <RadixDialog.Content className="fixed top-1/2 left-1/2 z-dialog flex max-h-full w-request-dialog max-w-full -translate-x-1/2 -translate-y-1/2 flex-col overflow-y-auto rounded-l bg-surface shadow-dialog outline-none">
          <div className="flex items-center justify-between gap-12 pt-18 pr-16 pb-6 pl-24">
            <RadixDialog.Title className="min-w-0 font-title text-21 leading-tight font-semibold text-text">
              Remove {target.name}?
            </RadixDialog.Title>
            <RadixDialog.Close asChild>
              <IconButton
                aria-label="Close"
                variant="ghost"
                disabled={remove.isPending}
              >
                <X aria-hidden="true" size={16} />
              </IconButton>
            </RadixDialog.Close>
          </div>
          <div className="flex flex-col gap-12 px-24 pt-8 pb-4">
            <RadixDialog.Description className="max-w-measure text-15 text-text">
              {target.host
                ? "fold stops sending notices to it from every project and deletes its URL"
                : "fold stops sending this project's notices to it and deletes its URL"}
              {target.kind === "webhook" ? " and its signing secret" : ""}. To
              send to it again, add it again with its URL.
            </RadixDialog.Description>
            {remove.isError ? (
              <ErrorNote failure={notifyFailure(remove.error)} />
            ) : null}
          </div>
          <div className="flex items-center justify-end gap-8 px-24 pt-18 pb-20">
            <Button disabled={remove.isPending} onClick={close}>
              Cancel
            </Button>
            <Button
              variant="danger"
              pending={remove.isPending}
              pendingLabel="Removing…"
              icon={<Trash2 aria-hidden="true" size={16} />}
              onClick={() => {
                remove.mutate();
              }}
            >
              Remove target
            </Button>
          </div>
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}

/** A person's switch of the target. It shows the daemon's value and stays pending until the settings are read again. */
function EnabledSwitch({
  scope,
  target,
  canWrite,
}: {
  scope: NotifyScope;
  target: NotifyTarget;
  canWrite: boolean;
}) {
  const refresh = useRefreshNotify();
  const save = useMutation({
    mutationFn: (enabled: boolean) =>
      editNotifyTarget(scope, target.id, { enabled }),
    onSuccess: refresh,
  });
  return (
    <div className="flex flex-col gap-6">
      <Checkbox
        label={`Send notices to ${target.name}`}
        hint={enabledHint(target, save.isPending)}
        checked={target.enabled}
        disabled={!canWrite || save.isPending}
        onCheckedChange={(on) => {
          save.mutate(on);
        }}
      />
      {save.isError ? <ErrorNote failure={notifyFailure(save.error)} /> : null}
    </div>
  );
}

function TargetRow({
  slug,
  target,
  usesHostTargets,
  canWrite,
  onSecret,
}: {
  slug: string;
  target: NotifyTarget;
  usesHostTargets: boolean;
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
  const { isSuccess: tested, reset: resetTest } = test;
  useEffect(() => {
    if (!tested) return;
    const timer = window.setTimeout(resetTest, TEST_NOTE_MS);
    return () => {
      window.clearTimeout(timer);
    };
  }, [tested, resetTest]);
  const rotate = useMutation({
    mutationFn: () => rotateNotifyTarget(scope, target.id),
    gcTime: 0,
    onSuccess: async (added) => {
      onSecret(added);
      await refresh();
      rotate.reset();
    },
  });
  const view = targetView(target, now, usesHostTargets);
  return (
    <li
      aria-label={view.title}
      className="flex flex-col gap-12 rounded-l bg-surface px-16 py-14 md:px-20"
    >
      <div className="flex flex-wrap items-start justify-between gap-12">
        <div className="flex min-w-0 flex-col gap-4">
          <p
            className={cn(
              "flex flex-wrap items-center gap-8 font-title text-16 leading-tight font-semibold",
              view.unused ? "text-text-2" : "text-text",
            )}
          >
            <span className="min-w-0 break-words">{view.title}</span>
            <Chip tone={CHIP_TONES[view.unused ? "muted" : view.state.tone]}>
              {view.state.label}
            </Chip>
            {view.health ? (
              <span className="font-body text-13 font-regular text-text-2">
                {view.health}
              </span>
            ) : null}
          </p>
          <p className="text-14 break-all text-text-2">
            <Id className="text-text">{target.id}</Id>
            {" · "}
            {view.detail}
          </p>
          <p className="text-13 text-text-2">{view.scope}</p>
        </div>
        <div className="flex min-w-0 flex-wrap items-center gap-8">
          <Button
            size="sm"
            disabled={!canWrite}
            pending={test.isPending}
            pendingLabel="Sending…"
            icon={<Send aria-hidden="true" size={14} />}
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
            aria-label={`Sends of ${target.name}`}
            icon={<ListOrdered aria-hidden="true" size={14} />}
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
            icon={<Pencil aria-hidden="true" size={14} />}
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
              icon={<KeyRound aria-hidden="true" size={14} />}
              aria-label={`New signing secret for ${target.name}`}
              onClick={() => {
                rotate.mutate();
              }}
            >
              New secret
            </Button>
          ) : null}
          <RemoveTarget scope={scope} target={target} canWrite={canWrite} />
        </div>
      </div>
      <EnabledSwitch scope={scope} target={target} canWrite={canWrite} />
      {test.isSuccess ? (
        <p role="status" className="flex items-center gap-6 text-15 text-ok">
          <CircleCheck aria-hidden="true" size={16} />
          The receiver took the test.
        </p>
      ) : null}
      {test.isError ? <ErrorNote failure={notifyFailure(test.error)} /> : null}
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

/** The targets that get the project's notices: its own, then the host's, each with its switch, test, send log, edit and removal. A host target shows while the project does not use the host targets too, greyed, so that a person can still change it. */
export function TargetList({
  slug,
  targets,
  usesHostTargets,
  canWrite,
  onSecret,
}: {
  slug: string;
  targets: readonly NotifyTarget[];
  usesHostTargets: boolean;
  canWrite: boolean;
  onSecret: (added: NotifyTargetAdded) => void;
}) {
  if (targets.length === 0) {
    return (
      <p className="text-15 text-text-2">No targets yet. Add one below.</p>
    );
  }
  return (
    <ul aria-label="Targets" className="flex flex-col gap-10">
      {targets.map((target) => (
        <TargetRow
          key={target.id}
          slug={slug}
          target={target}
          usesHostTargets={usesHostTargets}
          canWrite={canWrite}
          onSecret={onSecret}
        />
      ))}
    </ul>
  );
}
