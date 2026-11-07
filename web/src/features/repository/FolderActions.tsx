import { useIsMutating, useQuery } from "@tanstack/react-query";
import { RotateCcw, Undo2 } from "lucide-react";
import { useRef, useState } from "react";
import { resetFolder, restorePaths } from "@/api/folder";
import { needsYouQuery } from "@/api/queries";
import { Button } from "@/components/button/Button";
import { Mono } from "@/components/mono/Mono";
import { Dialog } from "@/components/overlay/Dialog";
import { FolderError } from "./FolderError";
import { focusAfterFolderWrite } from "./focus";
import {
  folderActions,
  resetPaths,
  resetText,
  restoreText,
} from "./folderReset";
import { LISTED_PATHS } from "./stash";
import { useFolderWrite } from "./useFolderWrite";

type Asking = "reset" | "restore";

function PathList({ paths }: { paths: readonly string[] }) {
  const listed = paths.slice(0, LISTED_PATHS * 3);
  const more = paths.length - listed.length;
  return (
    <ul className="flex flex-col gap-2">
      {listed.map((path) => (
        <li key={path} className="min-w-0">
          <Mono className="text-13 wrap-anywhere text-text">{path}</Mono>
        </li>
      ))}
      {more > 0 ? (
        <li className="text-13 text-text-2">and {String(more)} more</li>
      ) : null}
    </ul>
  );
}

/** Reset and Restore for a project folder that waits on the human's changes, as Needs you lists them: a waiting change, held files or an unstash conflict. Each asks first, names the safety stash, and stays pending until Needs you, the stash and the repository have been read again; then focus goes to the stash, which holds the safety entry, because these buttons are gone. */
export function FolderActions({ slug }: { slug: string }) {
  const needsYou = useQuery(needsYouQuery(slug));
  const [asking, setAsking] = useState<Asking | null>(null);
  /** Set once a write went through, so the dialog's close moves focus to the stash. */
  const done = useRef(false);
  const actions = folderActions(needsYou.data ?? []);
  const write = useFolderWrite(slug, async (asked: Asking) => {
    await (asked === "reset"
      ? resetFolder(slug)
      : restorePaths(slug, actions?.heldPaths ?? []));
    // Set before the reads that follow, which can remove this component and its dialog.
    done.current = true;
  });
  const writing = useIsMutating({ mutationKey: [slug, "folder-write"] }) > 0;
  if (actions === null) {
    return null;
  }
  const reset = resetText(actions);
  const restore = restoreText(actions.heldPaths);
  const canRestore =
    actions.heldPaths.length > 0 &&
    actions.markers.length === 0 &&
    actions.heldChange === null;
  const close = () => {
    if (!write.isPending) {
      setAsking(null);
      write.reset();
    }
  };
  return (
    <div className="flex flex-wrap gap-8">
      <Button
        size="sm"
        icon={<RotateCcw aria-hidden size={14} />}
        disabled={writing}
        onClick={() => {
          setAsking("reset");
        }}
      >
        {reset.label}
      </Button>
      {canRestore ? (
        <Button
          size="sm"
          variant="ghost"
          icon={<Undo2 aria-hidden size={14} />}
          disabled={writing}
          onClick={() => {
            setAsking("restore");
          }}
        >
          Restore held files
        </Button>
      ) : null}
      <Dialog
        open={asking !== null}
        onOpenChange={(open) => {
          if (!open) close();
        }}
        onCloseAutoFocus={(event) => {
          if (done.current && focusAfterFolderWrite(document, null)) {
            event.preventDefault();
          }
          done.current = false;
        }}
        title={asking === "restore" ? restore.title : reset.title}
        description={
          asking === "restore" ? restore.description : reset.description
        }
        footer={
          <div className="flex gap-8">
            <Button
              variant="danger"
              pending={write.isPending}
              pendingLabel={asking === "restore" ? "Restoring…" : "Resetting…"}
              onClick={() => {
                if (asking) {
                  write.mutate(asking, {
                    onSuccess: () => {
                      setAsking(null);
                    },
                  });
                }
              }}
            >
              {asking === "restore" ? "Restore files" : reset.label}
            </Button>
            <Button variant="ghost" disabled={write.isPending} onClick={close}>
              Cancel
            </Button>
          </div>
        }
      >
        {asking === "restore" ? <PathList paths={actions.heldPaths} /> : null}
        {asking === "reset" && resetPaths(actions).length > 0 ? (
          <PathList paths={resetPaths(actions)} />
        ) : null}
        <FolderError error={write.error} />
      </Dialog>
    </div>
  );
}
