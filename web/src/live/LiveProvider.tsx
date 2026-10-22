import { useQueryClient } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { useCallback, useEffect, useRef, type ReactNode } from "react";
import { openEventStream } from "../api/events";
import { redirectToSignIn } from "../app/authRedirect";
import { forgetProject } from "../app/projectGone";
import { keys } from "../api/keys";
import { cardTitle } from "../features/notifications/moves";
import { notificationStore } from "../features/notifications/store";
import { connectProject } from "./connect";
import { LiveRetryContext } from "./retry";
import { liveStatusStore } from "./status";
import type { StreamHandle } from "./stream";

/** Opens one event stream for the open project and closes it when the project changes or the layout unmounts. */
export function LiveProvider({
  slug,
  children,
}: {
  slug: string;
  children: ReactNode;
}) {
  const queryClient = useQueryClient();
  const router = useRouter();
  const stream = useRef<StreamHandle | null>(null);
  useEffect(() => {
    const handle = connectProject(
      queryClient,
      slug,
      openEventStream(slug),
      liveStatusStore,
      Date.now,
      () => {
        redirectToSignIn(queryClient, router.state.location, (href) => {
          void router.navigate({ href, replace: true });
        });
      },
      () => {
        forgetProject(queryClient, slug, () => void router.invalidate());
      },
      (event, replay) => {
        notificationStore.receive(slug, event, replay, (task) =>
          cardTitle(queryClient.getQueryData(keys.board(slug)), task),
        );
      },
    );
    stream.current = handle;
    return () => {
      handle.stop();
      notificationStore.leave(slug);
      if (stream.current === handle) {
        stream.current = null;
      }
    };
  }, [queryClient, router, slug]);
  const retry = useCallback(() => {
    stream.current?.retry();
  }, []);
  return <LiveRetryContext value={retry}>{children}</LiveRetryContext>;
}
