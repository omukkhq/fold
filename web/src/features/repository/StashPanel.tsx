import { useIsMutating, useQuery, useQueryClient } from "@tanstack/react-query";
import { Archive, Trash2 } from "lucide-react";
import { useRef, useState } from "react";
import { stashQuery } from "@/api/queries";
import { clearStash, dropStashEntry, type StashSeen } from "@/api/stash";
import { Button } from "@/components/button/Button";
import { CodeText } from "@/components/code/CodeText";
import { Id } from "@/components/id/Id";
import { Mono } from "@/components/mono/Mono";
import { Dialog } from "@/components/overlay/Dialog";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { failureText } from "@/format/failure";
import { ActorText } from "@/people/ActorText";
import { seg, actorSegment } from "@/people/segments";
import {
  CONFLICTED_NOTE,
  canClear,
  clearDescription,
  clearSeen,
  clearTitle,
  dropDescription,
  dropTitle,
  rowStillThere,
  stashRows,
  stashStillSeen,
  type StashRow,
} from "./stash";
import { stashChanged, stashChangedError } from "./errors";
import { FolderError } from "./FolderError";
import { STASH_HEADING_ID, focusAfterFolderWrite } from "./focus";
import { useFolderWrite } from "./useFolderWrite";

/** What the open dialog drops: one entry, or every entry. */
type Asking = { kind: "one"; row: StashRow } | { kind: "all"; seen: StashSeen };

/** The human's stash of the project folder, newest first, with a drop action per entry and Clear all. Each drop asks first, reads the stash again and refuses when it no longer holds what the person confirmed, and names what the person saw so the daemon refuses the same, and stays pending until the stash has been read again. Nothing renders while the stash is empty or its read has not arrived; a failed read shows its error. */
export function StashPanel({ slug, now }: { slug: string; now: number }) {
  const stash = useQuery(stashQuery(slug));
  const queryClient = useQueryClient();
  const [asking, setAsking] = useState<Asking | null>(null);
  /** Set by a drop that went through: the place of the dropped entry, or null after Clear all. */
  const dropped = useRef<number | null | undefined>(undefined);
  const drop = useFolderWrite(slug, async (asked: Asking) => {
    const current = await queryClient.query({
      ...stashQuery(slug),
      staleTime: 0,
    });
    const same =
      asked.kind === "one"
        ? rowStillThere(current.entries, asked.row)
        : stashStillSeen(current.entries, asked.seen);
    if (!same) {
      throw stashChangedError();
    }
    await (asked.kind === "one"
      ? dropStashEntry(slug, asked.row.position, asked.row.number)
      : clearStash(slug, asked.seen));
    // Set before the reads that follow, which can remove the row that opened the dialog.
    dropped.current = asked.kind === "one" ? asked.row.position : null;
  });
  const writing = useIsMutating({ mutationKey: [slug, "folder-write"] }) > 0;
  if (stash.isError) {
    return (
      <ErrorBanner
        className="rounded-m border-b-0 px-12"
        title="fold could not read the stash."
        message={failureText(stash.error)}
      />
    );
  }
  const entries = stash.data?.entries ?? [];
  if (entries.length === 0) {
    return null;
  }
  const rows = stashRows(entries, now);
  const changed = stashChanged(drop.error);
  const close = () => {
    if (!drop.isPending) {
      setAsking(null);
      drop.reset();
    }
  };
  return (
    <section
      aria-label="Stash"
      className="flex flex-col gap-8 rounded-m bg-surface px-16 py-12"
    >
      <div className="flex flex-wrap items-center justify-between gap-8">
        <h2
          id={STASH_HEADING_ID}
          tabIndex={-1}
          className="flex items-center gap-8 rounded-s font-title text-15 leading-tight font-semibold text-text"
        >
          <Archive aria-hidden size={15} className="text-text-2" />
          Stash
        </h2>
        <Button
          size="sm"
          variant="ghost"
          icon={<Trash2 aria-hidden size={14} />}
          disabled={!canClear(rows) || writing}
          onClick={() => {
            setAsking({ kind: "all", seen: clearSeen(rows) });
          }}
        >
          Clear all
        </Button>
      </div>
      <ul className="flex flex-col gap-10">
        {rows.map((row) => (
          <li
            key={row.key}
            className="flex flex-wrap items-start justify-between gap-8"
          >
            <div className="flex min-w-0 flex-col gap-2">
              <span className="text-14 text-text">
                <Id className="font-semibold">{row.name}</Id>{" "}
                <span className="text-text-2">
                  <ActorText
                    segments={seg`${row.files} · base ${row.base} · by ${actorSegment(row.actor)} · ${row.when}`}
                  />
                </span>
              </span>
              <span className="flex flex-wrap gap-x-10 gap-y-2">
                {row.paths.map((path) => (
                  <Mono
                    key={path}
                    className="min-w-0 text-13 wrap-anywhere text-text-2"
                  >
                    {path}
                  </Mono>
                ))}
              </span>
              {row.conflicted ? (
                <span className="text-13 text-text-2">
                  <CodeText text={CONFLICTED_NOTE} caption />
                </span>
              ) : null}
            </div>
            {row.conflicted ? null : (
              <Button
                size="sm"
                variant="ghost"
                icon={<Trash2 aria-hidden size={14} />}
                disabled={writing}
                aria-label={`Drop ${row.name}`}
                data-stash-position={row.position}
                onClick={() => {
                  setAsking({ kind: "one", row });
                }}
              >
                Drop
              </Button>
            )}
          </li>
        ))}
      </ul>
      <Dialog
        open={asking !== null}
        onOpenChange={(open) => {
          if (!open) close();
        }}
        onCloseAutoFocus={(event) => {
          if (
            dropped.current !== undefined &&
            focusAfterFolderWrite(document, dropped.current)
          ) {
            event.preventDefault();
          }
          dropped.current = undefined;
        }}
        title={
          asking?.kind === "one"
            ? dropTitle(asking.row)
            : clearTitle(asking?.seen.entries ?? 0)
        }
        description={
          asking?.kind === "one"
            ? dropDescription(asking.row)
            : clearDescription(asking?.seen.entries ?? 0)
        }
        footer={
          // After a refusal that says the stack moved, the question names an entry that may be gone, so only the way back to the list stays.
          changed ? (
            <Button variant="ghost" onClick={close}>
              Back to the list
            </Button>
          ) : (
            <div className="flex gap-8">
              <Button
                variant="danger"
                pending={drop.isPending}
                pendingLabel="Dropping…"
                onClick={() => {
                  if (asking) {
                    drop.mutate(asking, {
                      onSuccess: () => {
                        setAsking(null);
                      },
                    });
                  }
                }}
              >
                {asking?.kind === "all" ? "Drop all" : "Drop entry"}
              </Button>
              <Button variant="ghost" disabled={drop.isPending} onClick={close}>
                Cancel
              </Button>
            </div>
          )
        }
      >
        {drop.error === null ? null : <FolderError error={drop.error} />}
      </Dialog>
    </section>
  );
}
