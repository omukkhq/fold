import { useQueryClient } from "@tanstack/react-query";
import { useEffect, type ReactNode } from "react";
import { openEventStream } from "../api/events";
import { connectProject } from "./connect";
import { liveStatusStore } from "./status";

/** Opens one event stream for the open project and closes it when the project changes or the layout unmounts. */
export function LiveProvider({
  slug,
  children,
}: {
  slug: string;
  children: ReactNode;
}) {
  const queryClient = useQueryClient();
  useEffect(() => {
    const handle = connectProject(
      queryClient,
      slug,
      openEventStream(slug),
      liveStatusStore,
    );
    return () => {
      handle.stop();
    };
  }, [queryClient, slug]);
  return children;
}
