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
          "-mx-6 flex items-center gap-10 rounded-s px-6 py-3 hover:bg-fill/60",
          focusRing,
        )}
      >
        <span
          className={cn(
            "inline-flex size-20 shrink-0 items-center justify-center rounded-s font-mono text-13 font-semibold",
            kind.tone,
          )}
        >
          <span aria-hidden="true">{kind.letter}</span>
          <span className="sr-only">{kind.label}</span>
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-13 text-text">
          {fileLabel(file)}
        </span>
        {file.binary ? (
          <span className="font-mono text-13 text-text-2">binary</span>
        ) : (
          <>
            <span className="font-mono text-13 text-ok">
              +{file.added_lines}
            </span>
            <span className="font-mono text-13 text-bad">
              {"−"}
              {file.removed_lines}
            </span>
          </>
        )}
      </HrefLink>
    </li>
  );
}
