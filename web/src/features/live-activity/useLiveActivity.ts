import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState, useSyncExternalStore } from "react";
import { keys } from "../../api/keys";
import { listWorkspaces } from "../../api/workspaces";
import { useLiveStatus, type LiveStatus } from "../../live/status";
import { useDataClock } from "../../live/useDataClock";
import { createLiveStatesStore } from "./liveStates";
import { orderPanels } from "./order";
import { buildPanels, lastActivity, seedLive, type Panel } from "./panels";

export interface LiveActivity {
  status: "pending" | "error" | "ready";
  panels: Panel[];
  live: LiveStatus;
  /** The time the screen last rendered new data, for its relative times; it never ticks. */
  now: number;
  retry: () => void;
}

export function useLiveActivity(slug: string): LiveActivity {
  const queryClient = useQueryClient();
  const live = useLiveStatus();
  const list = useQuery({
    queryKey: keys.workspaces(slug),
    queryFn: async () => {
      const entries = await listWorkspaces(slug);
      seedLive(queryClient, slug, entries);
      return entries;
    },
  });
  const entries = list.data;
  const store = useMemo(
    () => createLiveStatesStore(queryClient, slug),
    [queryClient, slug],
  );
  useSyncExternalStore(store.subscribe, store.getVersion, store.getVersion);
  const unordered = buildPanels(
    entries ?? [],
    (entries ?? []).map((entry) => store.read(entry.workspace)),
  );
  const [order, setOrder] = useState<string[]>([]);
  const next = orderPanels(
    order,
    unordered.map((panel) => ({
      id: panel.entry.workspace,
      lastActivityAt: lastActivity(panel),
    })),
  );
  if (next.join() !== order.join()) {
    setOrder(next);
  }
  const panels = next.flatMap((id) =>
    unordered.filter((panel) => panel.entry.workspace === id),
  );
  const now = useDataClock([list.dataUpdatedAt, live.lastEventAt]);
  return {
    status: list.isError
      ? "error"
      : entries === undefined
        ? "pending"
        : "ready",
    panels,
    live,
    now,
    retry: () => void list.refetch(),
  };
}
