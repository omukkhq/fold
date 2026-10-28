import { useMutation, useQuery } from "@tanstack/react-query";
import { Check, Copy, KeyRound, RefreshCw, Save, X } from "lucide-react";
import { useState, type ReactNode, type SyntheticEvent } from "react";
import { apiErrorCode } from "@/api/errors";
import {
  setUseHostTargets,
  setWebUrl,
  type NotifyTargetAdded,
  type ProjectNotify,
} from "@/api/notify";
import { projectNotifyQuery, sessionQuery } from "@/api/queries";
import { Button, IconButton } from "@/components/button/Button";
import { Checkbox, TextField } from "@/components/form/controls";
import { copyOutcome, type CopyState } from "@/components/mono/commitCopy";
import { Mono } from "@/components/mono/Mono";
import { FailurePanel } from "@/components/states/FailurePanel";
import { Skeleton } from "@/components/states/Skeleton";
import { ErrorNote } from "../workers/parts";
import {
  checkWebUrl,
  hostTargetsHint,
  notifyFailure,
  projectStatus,
  webUrlHint,
} from "./notifyModel";
import { TargetForm } from "./TargetForm";
import { TargetList, useRefreshNotify } from "./TargetList";

function Section({
  title,
  text,
  children,
}: {
  title: string;
  text: string;
  children: ReactNode;
}) {
  return (
    <section
      aria-label={title}
      className="flex flex-col gap-14 rounded-l bg-surface px-20 py-18 inset-ring inset-ring-line"
    >
      <div className="flex flex-col gap-4">
        <h2 className="text-16 leading-tight font-semibold text-text">
          {title}
        </h2>
        <p className="text-14 text-text-2">{text}</p>
      </div>
      {children}
    </section>
  );
}

/** The signing secret of an add or a rotation, which the daemon shows only once: it lives in this panel's state until the person closes it. */
function SecretPanel({
  added,
  onClose,
}: {
  added: NotifyTargetAdded;
  onClose: () => void;
}) {
  const [copy, setCopy] = useState<CopyState>("idle");
  const secret = added.secret ?? "";
  return (
    <div className="flex items-start justify-between gap-12 rounded-m bg-warn-soft px-14 py-12">
      <div className="flex min-w-0 flex-col gap-6">
        <p className="flex items-center gap-6 text-14 font-semibold text-warn">
          <KeyRound aria-hidden="true" size={14} />
          Signing secret of {added.target.name}
        </p>
        <Mono className="text-13 break-all text-text">{secret}</Mono>
        <p className="text-13 text-text-2">
          fold shows it only now. Give it to the receiver, which checks the
          webhook-signature header with it.
        </p>
      </div>
      <div className="flex shrink-0 items-center gap-4">
        <IconButton
          aria-label="Copy the signing secret"
          onClick={() => {
            void copyOutcome(secret, navigator.clipboard).then(setCopy);
          }}
        >
          {copy === "copied" ? (
            <Check aria-hidden="true" size={15} />
          ) : (
            <Copy aria-hidden="true" size={15} />
          )}
        </IconButton>
        <span role="status" className="sr-only">
          {copy === "copied" ? "Copied" : copy === "failed" ? "Not copied" : ""}
        </span>
        <IconButton aria-label="Close" onClick={onClose}>
          <X aria-hidden="true" size={15} />
        </IconButton>
      </div>
    </div>
  );
}

function HostTargetsSwitch({
  slug,
  settings,
  canWrite,
}: {
  slug: string;
  settings: ProjectNotify;
  canWrite: boolean;
}) {
  const refresh = useRefreshNotify();
  const save = useMutation({
    mutationFn: (on: boolean) => setUseHostTargets(slug, on),
    onSuccess: refresh,
  });
  return (
    <div className="flex flex-col gap-6">
      <Checkbox
        label="Use the host targets"
        hint={save.isPending ? "Saving…" : hostTargetsHint(settings)}
        checked={settings.use_host_targets}
        disabled={!canWrite || save.isPending}
        onCheckedChange={(on) => {
          save.mutate(on);
        }}
      />
      {save.isError ? <ErrorNote failure={notifyFailure(save.error)} /> : null}
    </div>
  );
}

function WebUrlForm({
  webUrl,
  canWrite,
}: {
  webUrl: string | null;
  canWrite: boolean;
}) {
  const [text, setText] = useState(webUrl ?? "");
  const [error, setError] = useState<string | undefined>();
  const refresh = useRefreshNotify();
  const save = useMutation({
    mutationFn: (value: string | null) => setWebUrl(value),
    onSuccess: refresh,
  });
  const submit = (event: SyntheticEvent) => {
    event.preventDefault();
    const checked = checkWebUrl(text);
    if (checked.error !== undefined) {
      setError(checked.error);
      return;
    }
    setError(undefined);
    save.mutate(checked.webUrl);
  };
  return (
    <form
      noValidate
      aria-label="Web address of links"
      onSubmit={submit}
      className="flex flex-wrap items-end gap-10"
    >
      <div className="min-w-0 flex-1">
        <TextField
          label="Web address of links"
          hint={webUrlHint(webUrl)}
          inputMode="url"
          autoComplete="off"
          value={text}
          error={error}
          disabled={!canWrite || save.isPending}
          onChange={(event) => {
            setText(event.target.value);
          }}
        />
      </div>
      <Button
        type="submit"
        disabled={!canWrite}
        pending={save.isPending}
        pendingLabel="Saving…"
        icon={<Save aria-hidden="true" size={15} />}
      >
        Save
      </Button>
      {save.isError ? (
        <ErrorNote className="w-full" failure={notifyFailure(save.error)} />
      ) : null}
    </form>
  );
}

/** Where fold sends the project's notices: its own targets and the host's, each with the events it selected. */
export function SettingsScreen({ slug }: { slug: string }) {
  const project = useQuery(projectNotifyQuery(slug));
  const session = useQuery(sessionQuery());
  const canWrite = (session.data?.human ?? null) !== null;
  const refresh = useRefreshNotify();
  const [secret, setSecret] = useState<NotifyTargetAdded | null>(null);
  const [host, setHost] = useState(false);
  const showSecret = (added: NotifyTargetAdded | null) => {
    if (added?.secret) setSecret(added);
  };
  return (
    <div className="px-24 pt-20 pb-24">
      <div className="mx-auto flex w-content max-w-full flex-col gap-16">
        <div className="flex flex-col gap-4">
          <h1 className="text-24 leading-tight font-semibold text-text">
            Settings
          </h1>
          <p className="text-15 text-text-2">
            fold sends a notice to a webhook, Slack or Discord when something on
            the project changes. A notice names ids, titles, columns and links,
            never a message, a review text or a diff.
          </p>
          {canWrite ? null : (
            <p className="text-14 text-warn">
              Sign in as a person first: only a person changes where notices go.
            </p>
          )}
        </div>
        {secret ? (
          <SecretPanel
            key={secret.secret}
            added={secret}
            onClose={() => {
              setSecret(null);
            }}
          />
        ) : null}
        <Section
          title="Notices"
          text={
            project.data
              ? projectStatus(project.data)
              : "The targets of this project and of the host."
          }
        >
          {project.isError ? (
            <FailurePanel
              title="Could not load the notice settings"
              text={notifyFailure(project.error).text}
              code={apiErrorCode(project.error)}
              action={
                <Button
                  icon={<RefreshCw aria-hidden="true" size={15} />}
                  onClick={() => void project.refetch()}
                >
                  Try again
                </Button>
              }
            />
          ) : project.data === undefined ? (
            <Skeleton />
          ) : (
            <>
              <HostTargetsSwitch
                slug={slug}
                settings={project.data}
                canWrite={canWrite}
              />
              <TargetList
                slug={slug}
                targets={[
                  ...project.data.targets,
                  ...project.data.host_targets,
                ]}
                usesHostTargets={project.data.use_host_targets}
                canWrite={canWrite}
                onSecret={showSecret}
              />
              <Checkbox
                label="Add the new target to the host"
                hint="A host target gets the notices of every project that uses the host targets."
                checked={host}
                disabled={!canWrite}
                onCheckedChange={setHost}
              />
              <TargetForm
                scope={host ? { kind: "host" } : { kind: "project", slug }}
                canWrite={canWrite}
                onSaved={async (added) => {
                  showSecret(added);
                  await refresh();
                }}
              />
              <WebUrlForm
                key={project.data.web_url ?? ""}
                webUrl={project.data.web_url}
                canWrite={canWrite}
              />
            </>
          )}
        </Section>
      </div>
    </div>
  );
}
