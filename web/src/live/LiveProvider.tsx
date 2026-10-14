import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, type ReactNode } from "react";
import { openEventStream } from "../api/events";
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
  const stream = useRef<StreamHandle | null>(null);
  useEffect(() => {
    const handle = connectProject(
      queryClient,
      slug,
      openEventStream(slug),
      liveStatusStore,
    );
    stream.current = handle;
    return () => {
      handle.stop();
      if (stream.current === handle) {
        stream.current = null;
      }
    };
  }, [queryClient, slug]);
  const retry = useCallback(() => {
    stream.current?.retry();
  }, []);
  return <LiveRetryContext value={retry}>{children}</LiveRetryContext>;
}
