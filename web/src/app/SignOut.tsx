import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { Loader2, LogOut } from "lucide-react";
import { signOut } from "../api/session";
import { failureText } from "../format/failure";
import { cn } from "../lib/cn";
import { insetFocusRing } from "../lib/focus";
import { leaveAfterSignOut } from "./signOutModel";

/** A menu row that deletes the session; it stays pending until the daemon answers, then opens the sign-in screen. */
export function SignOut() {
  const queryClient = useQueryClient();
  const router = useRouter();
  const write = useMutation({
    mutationFn: signOut,
    onSuccess: () =>
      leaveAfterSignOut(queryClient, (href) => router.navigate({ href })),
  });
  return (
    <>
      <button
        type="button"
        disabled={write.isPending}
        aria-busy={write.isPending ? true : undefined}
        onClick={() => {
          write.mutate();
        }}
        className={cn(
          "flex min-h-44 w-full items-center gap-10 px-16 text-left text-14 text-text hover:bg-sunken disabled:cursor-not-allowed",
          insetFocusRing,
        )}
      >
        {write.isPending ? (
          <Loader2
            aria-hidden="true"
            size={16}
            className="animate-spin text-text-2 motion-reduce:animate-none"
          />
        ) : (
          <LogOut aria-hidden="true" size={16} className="text-text-2" />
        )}
        {write.isPending ? "Signing out…" : "Sign out"}
      </button>
      {write.error === null ? null : (
        <p role="alert" className="px-16 pb-8 text-13 text-bad">
          {failureText(write.error)}
        </p>
      )}
    </>
  );
}
