import type { LiveWorkspace } from "../../live/envelope";
import { cn } from "../../lib/cn";
import { focusRing } from "../../lib/focus";
import { HrefLink } from "../../app/HrefLink";
import { changeKind, fileLabel } from "./files";
import { workingFileHref } from "./links";

interface ChangedFileRowProps {
  slug: string;
  base: string;
  workspace: string;
  file: LiveWorkspace["files"][number];
}

export function ChangedFileRow({
  slug,
  base,
  workspace,
  file,
}: ChangedFileRowProps) {
  const kind = changeKind(file.change);
  return (
    <li>
      <HrefLink
        href={workingFileHref(slug, base, workspace, file.path)}
        className={cn(
          "flex items-center gap-10 rounded-s px-4 py-3 hover:bg-sunken",
          focusRing,
        )}
      >
        <span
          title={kind.label}
          className="inline-flex size-18 shrink-0 items-center justify-center rounded-s bg-sunken font-mono text-caption font-bold text-text-muted"
        >
          <span aria-hidden="true">{kind.letter}</span>
          <span className="sr-only">{kind.label}</span>
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-meta text-text">
          {fileLabel(file)}
        </span>
        {file.binary ? (
          <span className="font-mono text-meta text-text-muted">binary</span>
        ) : (
          <>
            <span className="font-mono text-meta text-primary-text">
              +{file.added_lines}
            </span>
            <span className="font-mono text-meta text-danger">
              {"−"}
              {file.removed_lines}
            </span>
          </>
        )}
      </HrefLink>
    </li>
  );
}
