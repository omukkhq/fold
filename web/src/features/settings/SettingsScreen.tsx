import { useMutation, useQuery } from "@tanstack/react-query";
import { RefreshCw, Save } from "lucide-react";
import { useState, type SyntheticEvent, type ReactNode } from "react";
import { apiErrorCode } from "@/api/errors";
import { setWebUrl, type NotifyScope } from "@/api/notify";
import {
  hostNotifyQuery,
  projectNotifyQuery,
  sessionQuery,
} from "@/api/queries";
import { Button } from "@/components/button/Button";
import { TextField } from "@/components/form/controls";
import { Mono } from "@/components/mono/Mono";
import { FailurePanel } from "@/components/states/FailurePanel";
import { Skeleton } from "@/components/states/Skeleton";
import { ErrorNote } from "../workers/parts";
import { AddTargetForm } from "./AddTargetForm";
import { NotifyTargets, useRefreshNotify } from "./NotifyTargets";
import { checkWebUrl, notifyFailure, projectStatus } from "./notifyModel";

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
      className="flex flex-col gap-14 rounded-l bg-surface px-20 py-18 inset-ring inset-ring-border-subtle"
    >
      <div className="flex flex-col gap-4">
        <h2 className="text-section leading-snug font-semibold text-text">
          {title}
        </h2>
        <p className="text-bar text-text-muted">{text}</p>
      </div>
      {children}
    </section>
  );
}

const HOST: NotifyScope = { kind: "host" };

/** The web address that links start with. The field starts empty when links follow the listen address. */
function WebUrlForm({
  webUrl,
  linkBase,
  canWrite,
}: {
  webUrl: string | null;
  linkBase: string;
  canWrite: boolean;
}) {
  const [text, setText] = useState(webUrl ?? "");
  const [error, setError] = useState<string | undefined>();
  const refresh = useRefreshNotify(HOST);
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
      aria-label="Link address"
      onSubmit={submit}
      className="flex flex-col gap-10"
    >
      <div className="flex flex-wrap items-end gap-10">
        <div className="min-w-0 flex-1">
          <TextField
            label="Address of the web app in links"
            hint="The address people open fold at. Empty: the daemon's listen address."
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
      </div>
      <p className="text-meta text-text-muted">
        Links start with <Mono className="text-meta text-text">{linkBase}</Mono>
      </p>
      {save.isError ? <ErrorNote failure={notifyFailure(save.error)} /> : null}
    </form>
  );
}

function LoadFailure({
  error,
  onRetry,
}: {
  error: unknown;
  onRetry: () => void;
}) {
  return (
    <FailurePanel
      title="Could not load the notification settings"
      text={notifyFailure(error).text}
      code={apiErrorCode(error)}
      action={
        <Button
          icon={<RefreshCw aria-hidden="true" size={15} />}
          onClick={onRetry}
        >
          Try again
        </Button>
      }
    />
  );
}

/** Where fold sends a notice when an item comes into Needs you: this project's targets, and the host's that every project without targets of its own uses. */
export function SettingsScreen({ slug }: { slug: string }) {
  const project = useQuery(projectNotifyQuery(slug));
  const host = useQuery(hostNotifyQuery());
  const session = useQuery(sessionQuery());
  const canWrite = (session.data?.human ?? null) !== null;
  const scope: NotifyScope = { kind: "project", slug };
  return (
    <div className="px-24 pt-20 pb-24">
      <div className="mx-auto flex w-content max-w-full flex-col gap-16">
        <div className="flex flex-col gap-4">
          <h1 className="text-screen leading-snug font-semibold text-text">
            Settings
          </h1>
          <p className="text-body text-text-muted">
            fold sends one notice when an item comes into Needs you, and one
            digest when several come together. A notice names each item with a
            short title and a link, never a message or a diff.
          </p>
          {canWrite ? null : (
            <p className="text-bar text-warning">
              Tell fold your name first: only a named human changes where
              notices go.
            </p>
          )}
        </div>
        <Section
          title="This project's notices"
          text={
            project.data
              ? projectStatus(project.data)
              : "Targets of this project only."
          }
        >
          {project.isError ? (
            <LoadFailure
              error={project.error}
              onRetry={() => void project.refetch()}
            />
          ) : project.data === undefined ? (
            <Skeleton />
          ) : (
            <>
              <NotifyTargets
                scope={scope}
                targets={project.data.targets}
                canWrite={canWrite}
                label="Project targets"
              />
              <AddTargetForm scope={scope} canWrite={canWrite} />
            </>
          )}
        </Section>
        <Section
          title="Host targets"
          text="Every project without targets of its own sends its notices here."
        >
          {host.isError ? (
            <LoadFailure
              error={host.error}
              onRetry={() => void host.refetch()}
            />
          ) : host.data === undefined ? (
            <Skeleton />
          ) : (
            <>
              <NotifyTargets
                scope={HOST}
                targets={host.data.targets}
                canWrite={canWrite}
                label="Host targets"
              />
              {host.data.targets.length === 0 ? (
                <p className="text-bar text-text-muted">No host targets yet.</p>
              ) : null}
              <AddTargetForm scope={HOST} canWrite={canWrite} />
              <WebUrlForm
                key={host.data.web_url ?? ""}
                webUrl={host.data.web_url}
                linkBase={host.data.link_base}
                canWrite={canWrite}
              />
            </>
          )}
        </Section>
      </div>
    </div>
  );
}
