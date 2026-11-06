import { useIsMutating, useQuery } from "@tanstack/react-query";
import { Archive, Trash2 } from "lucide-react";
import { useState } from "react";
import { stashQuery } from "@/api/queries";
import { clearStash, dropStashEntry } from "@/api/stash";
import { Button } from "@/components/button/Button";
import { CodeText } from "@/components/code/CodeText";
import { Id } from "@/components/id/Id";
import { Mono } from "@/components/mono/Mono";
import { Dialog } from "@/components/overlay/Dialog";
import { WriteError } from "@/features/task/parts";
import { ActorText } from "@/people/ActorText";
import { seg, actorSegment } from "@/people/segments";
import {
  CONFLICTED_NOTE,
  canClear,
  clearDescription,
  clearTitle,
  dropDescription,
  dropTitle,
  stashRows,
  type StashRow,
} from "./stash";
import { useFolderWrite } from "./useFolderWrite";

/** What the open dialog drops: one entry, or every entry. */
type Asking = { kind: "one"; row: StashRow } | { kind: "all"; count: number };

/** The human's stash of the project folder, newest first, with a drop action per entry and Clear all. Each drop asks first and stays pending until the stash has been read again. Nothing renders while the stash is empty or its read has not arrived, because the repository screen carries its own states. */
export function StashPanel({ slug, now }: { slug: string; now: number }) {
  const stash = useQuery(stashQuery(slug));
  const [asking, setAsking] = useState<Asking | null>(null);
  const drop = useFolderWrite(slug, (asked: Asking) =>
    asked.kind === "one"
      ? dropStashEntry(slug, asked.row.position)
      : clearStash(slug),
  );
  const writing = useIsMutating({ mutationKey: [slug, "folder-write"] }) > 0;
  const entries = stash.data?.entries ?? [];
  if (entries.length === 0) {
    return null;
  }
  const rows = stashRows(entries, now);
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
        <h2 className="flex items-center gap-8 font-title text-15 leading-tight font-semibold text-text">
          <Archive aria-hidden size={15} className="text-text-2" />
          Stash
        </h2>
        <Button
          size="sm"
          variant="ghost"
          icon={<Trash2 aria-hidden size={14} />}
          disabled={!canClear(rows) || writing}
          onClick={() => {
            setAsking({ kind: "all", count: rows.length });
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
                  <Mono key={path} className="text-13 text-text-2">
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
        title={
          asking?.kind === "one"
            ? dropTitle(asking.row)
            : clearTitle(asking?.count ?? 0)
        }
        description={
          asking?.kind === "one"
            ? dropDescription(asking.row)
            : clearDescription(asking?.count ?? 0)
        }
        footer={
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
        }
      >
        <WriteError error={drop.error} />
      </Dialog>
    </section>
  );
}
