import { queryOptions } from "@tanstack/react-query";
import { api } from "./client";
import { keys } from "./keys";
import { unwrap } from "./result";

/** The names of the humans the host knows, without the `human:` prefix. */
export async function listHumans(): Promise<string[]> {
  const { humans } = await unwrap(api.GET("/api/v1/humans"));
  return humans;
}

const MINUTE_MS = 60_000;

/** A new person appears when they first sign in and no event announces it, so a list older than a minute is read again when a screen opens or the window takes focus. */
export const humansQuery = () =>
  queryOptions({
    queryKey: keys.humans(),
    queryFn: listHumans,
    staleTime: MINUTE_MS,
    refetchOnWindowFocus: true,
  });
