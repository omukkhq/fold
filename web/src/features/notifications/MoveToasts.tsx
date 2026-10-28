import * as Toast from "@radix-ui/react-toast";
import { SquareKanban, X } from "lucide-react";
import { createPortal } from "react-dom";
import { IdText } from "@/components/text/IdText";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { notificationStore, useMoveToasts } from "./store";

/** Below `md` a screen's own actions sit in a bar at the bottom, such as a proposal's Approve and Request changes, so the toasts show under the top bar there; from `md` they sit at the bottom right. */
export const TOAST_PLACE =
  "fixed inset-x-16 top-(--height-bar-compact) z-20 mt-8 flex flex-col gap-8 outline-none md:top-auto md:bottom-16 md:left-auto md:mt-0 md:w-toast";

/** A toast closes by itself after this time; hovering or focusing it holds it open. */
const TOAST_MS = 5_000;

/** A toast for each batch of card moves that arrives, at the bottom right; the text follows the toast rules of `text.ts`. */
export function MoveToasts({ slug }: { slug: string }) {
  const toasts = useMoveToasts().filter((toast) => toast.slug === slug);
  return (
    <Toast.Provider label="Card moves" duration={TOAST_MS}>
      {toasts.map((toast) => (
        <Toast.Root
          key={toast.id}
          type="background"
          onOpenChange={(open) => {
            if (!open) {
              notificationStore.dismiss(toast.id);
            }
          }}
          className="flex items-start gap-10 rounded-m bg-ink px-14 py-10 text-on-ink shadow-dialog data-[swipe=cancel]:translate-x-0 data-[swipe=cancel]:transition-transform data-[swipe=end]:translate-x-(--radix-toast-swipe-end-x) data-[swipe=end]:opacity-0 data-[swipe=end]:transition-[translate,opacity] data-[swipe=move]:translate-x-(--radix-toast-swipe-move-x) motion-reduce:transition-none"
        >
          <SquareKanban
            aria-hidden="true"
            size={16}
            className="mt-2 shrink-0"
          />
          <Toast.Description className="min-w-0 flex-1 break-words text-14">
            <IdText text={toast.text} />
          </Toast.Description>
          <Toast.Close
            aria-label="Dismiss"
            className={cn(
              "rounded-s p-2 text-on-ink/70 hover:text-on-ink",
              focusRing,
            )}
          >
            <X aria-hidden="true" size={14} />
          </Toast.Close>
        </Toast.Root>
      ))}
      {createPortal(
        <Toast.Viewport
          label="Card moves ({hotkey})"
          className={TOAST_PLACE}
        />,
        document.body,
      )}
    </Toast.Provider>
  );
}
