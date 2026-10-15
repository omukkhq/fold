import { useState } from "react";
import { Button } from "@/components/button/Button";
import { Dialog } from "@/components/overlay/Dialog";
import { failureText } from "@/format/failure";

interface IntegratorDialogProps {
  agent: string;
  /** True to grant the tag, false to remove it. */
  grant: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Resolves once the daemon has changed the tag; rejects with the daemon's error. */
  onConfirm: () => Promise<void>;
}

export function IntegratorDialog({
  agent,
  grant,
  open,
  onOpenChange,
  onConfirm,
}: IntegratorDialogProps) {
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  function close(next: boolean) {
    if (pending) {
      return;
    }
    setFailure(null);
    onOpenChange(next);
  }

  async function confirm() {
    setPending(true);
    setFailure(null);
    try {
      await onConfirm();
      setPending(false);
      onOpenChange(false);
    } catch (error) {
      setPending(false);
      setFailure(failureText(error));
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      title={
        grant
          ? `Grant integrator to ${agent}?`
          : `Remove integrator from ${agent}?`
      }
      description={
        grant
          ? `${agent} will be able to integrate approved proposals into main.`
          : `${agent} will no longer be able to integrate proposals into main.`
      }
    >
      {failure ? (
        <p role="alert" className="text-bar font-medium text-danger">
          The tag did not change. {failure}
        </p>
      ) : null}
      <div className="flex justify-end gap-8">
        <Button
          variant="ghost"
          disabled={pending}
          onClick={() => {
            close(false);
          }}
        >
          Cancel
        </Button>
        <Button
          variant={grant ? "primary" : "danger"}
          pending={pending}
          onClick={() => void confirm()}
        >
          {grant ? "Grant integrator" : "Remove integrator"}
        </Button>
      </div>
    </Dialog>
  );
}
