import { Archive, Ellipsis } from "lucide-react";
import { useState } from "react";
import { archiveTask } from "@/api/tasks";
import { Button, IconButton } from "@/components/button/Button";
import { Dialog } from "@/components/overlay/Dialog";
import { Menu } from "@/components/overlay/Menu";
import { WriteError } from "./parts";
import { useTaskWrite } from "./useTaskWrite";

/** W03's "…" menu in the dialog head. Archiving asks first, stays pending until the board has been read again and then closes the dialog. */
export function ArchiveMenu({
  slug,
  task,
  onArchived,
}: {
  slug: string;
  task: string;
  onArchived: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const archive = useTaskWrite(slug, task, () => archiveTask(slug, task));
  return (
    <>
      <Menu
        align="end"
        items={[
          {
            label: "Archive",
            icon: Archive,
            danger: true,
            onSelect: () => {
              setConfirming(true);
            },
          },
        ]}
        trigger={
          <IconButton aria-label="More actions">
            <Ellipsis aria-hidden="true" size={15} />
          </IconButton>
        }
      />
      <Dialog
        open={confirming}
        onOpenChange={(open) => {
          if (!archive.isPending) {
            setConfirming(open);
            if (!open) archive.reset();
          }
        }}
        title={`Archive ${task}?`}
        description="The task leaves the board and its dependencies are removed. Its history stays in the activity list."
      >
        <WriteError error={archive.error} />
        <div className="flex gap-8">
          <Button
            variant="danger"
            pending={archive.isPending}
            onClick={() => {
              archive.mutate(undefined, { onSuccess: onArchived });
            }}
          >
            Archive task
          </Button>
          <Button
            variant="ghost"
            disabled={archive.isPending}
            onClick={() => {
              setConfirming(false);
              archive.reset();
            }}
          >
            Cancel
          </Button>
        </div>
      </Dialog>
    </>
  );
}
