import { useMutation, useQuery } from "@tanstack/react-query";
import { Pencil, Save } from "lucide-react";
import { useState } from "react";
import {
  editProposalFile,
  type NewEdit,
  type ProposalDetail,
} from "@/api/proposals";
import { proposalFileQuery } from "@/api/queries";
import { Button } from "@/components/button/Button";
import { Textarea } from "@/components/form/controls";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import {
  changed,
  editIntent,
  editNotice,
  editRequest,
  editSource,
  intentProblem,
} from "./edit";
import { describeFailure } from "./errors";
import type { DetailView } from "./view";

interface FileEditorProps {
  slug: string;
  id: string;
  proposal: ProposalDetail;
  view: DetailView;
  path: string;
  onCancel: () => void;
  /** Resolves once the proposal with the new version is read again, so Save stays pending until the screen shows it. */
  onSaved: () => Promise<void>;
}

/** The edit box of one file of the current version, in a human review. */
export function FileEditor({
  slug,
  id,
  proposal,
  view,
  path,
  onCancel,
  onSaved,
}: FileEditorProps) {
  const file = useQuery(proposalFileQuery(slug, id, view.version, path));
  const [value, setValue] = useState<string | null>(null);
  const started = editIntent(proposal, view.version);
  const [intent, setIntent] = useState(started);
  const problem = intentProblem(intent, started);
  const save = useMutation({
    mutationFn: (edit: NewEdit) => editProposalFile(slug, id, edit),
    onSuccess: onSaved,
  });

  if (file.isPending) return <Skeleton className="min-w-0 flex-1" />;
  if (file.isError) {
    return (
      <ErrorBanner
        className="min-w-0 flex-1"
        title="The file could not load."
        message="The content of the file did not arrive."
        onRetry={() => void file.refetch()}
      />
    );
  }
  const source = editSource(file.data);
  const next = `v${String(view.version + 1)}`;
  const text = source.kind === "text" ? (value ?? source.text) : "";
  return (
    <section
      aria-label={`Edit ${path}`}
      className="flex min-w-0 flex-1 flex-col gap-10 rounded-m bg-surface p-16"
    >
      <h2 className="flex items-center gap-8 text-14 font-semibold text-text">
        <Pencil aria-hidden="true" size={15} className="shrink-0" />
        <span className="min-w-0 font-mono break-all">{path}</span>
      </h2>
      {source.kind === "blocked" ? (
        <p className="text-13 leading-tight text-text-2">{source.reason}</p>
      ) : (
        <>
          <p className="text-13 leading-tight text-text-2">
            {editNotice(view.version)}
          </p>
          <Textarea
            label={`Content of ${path}`}
            rows={20}
            spellCheck={false}
            readOnly={save.isPending}
            value={text}
            onChange={(event) => {
              setValue(event.target.value);
            }}
            className="resize-y font-mono text-13"
          />
          <Textarea
            label={`Intent of ${next}`}
            hint={`What ${next} does, why, what it does not change and its risks. It starts as the intent of v${String(view.version)}; change what your edit changes.`}
            error={problem ?? undefined}
            rows={4}
            readOnly={save.isPending}
            value={intent}
            onChange={(event) => {
              setIntent(event.target.value);
            }}
            className="resize-y"
          />
        </>
      )}
      {save.isError ? (
        <p role="alert" className="text-13 font-medium text-bad">
          {describeFailure(save.error).text}
        </p>
      ) : null}
      <div className="flex gap-8">
        {source.kind === "text" ? (
          <Button
            variant="primary"
            pending={save.isPending}
            pendingLabel={`Saving ${next}…`}
            disabled={!changed(source, text) || problem !== null}
            icon={<Save aria-hidden="true" size={15} />}
            onClick={() => {
              save.mutate(
                editRequest(view, path, text, source, intent, started),
              );
            }}
          >
            Save as {next}
          </Button>
        ) : null}
        <Button variant="ghost" disabled={save.isPending} onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </section>
  );
}
