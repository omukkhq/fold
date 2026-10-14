export type Presence = "active" | "idle" | "offline";

export const PRESENCE_STATES: readonly Presence[] = [
  "active",
  "idle",
  "offline",
];

const SPECS: Record<Presence, { label: string; dot: string }> = {
  active: { label: "Active", dot: "bg-presence-active" },
  idle: { label: "Idle", dot: "bg-presence-idle" },
  offline: { label: "Offline", dot: "bg-presence-offline" },
};

/** The accessible name and dot class of a presence state. */
export function presenceSpec(state: Presence): { label: string; dot: string } {
  return SPECS[state];
}
