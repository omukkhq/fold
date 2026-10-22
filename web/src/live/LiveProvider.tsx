import { useQueryClient } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { useCallback, useEffect, useRef, type ReactNode } from "react";
import { openEventStream } from "../api/events";
import { redirectToSignIn } from "../app/authRedirect";
import { forgetProject } from "../app/projectGone";
import { listEventPage } from "../api/events";
import { keys } from "../api/keys";
import type { Session } from "../api/session";
import { cardTitle } from "../features/notifications/moves";
import { HISTORY_PAGE } from "../features/notifications/history";
import { notificationStore } from "../features/notifications/store";
import { reviewerActor } from "../people/names";
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
    const title = (task: string) =>
      cardTitle(queryClient.getQueryData(keys.board(slug)), task);
    const self = () =>
      reviewerActor(
        queryClient.getQueryData<Session>(keys.session())?.human ?? null,
      );
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
        notificationStore.receive(slug, event, { replay, title, self: self() });
      },
    );
    void notificationStore.fill(slug, {
      read: (before) => listEventPage(slug, before, HISTORY_PAGE),
      title,
      self,
    });
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
