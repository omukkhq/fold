import type { BoardLabel } from "@/api/board";
import type { AreaEntry } from "@/api/workers";

/** The area field's value when the task gets no area label. */
export const NO_AREA = "";

/** The area choice that gives a task no area label, so the main agent chooses its area. */
export const NO_AREA_TEXT = "Decide automatically";

/** The area menu's value when it marks no choice. An area name starts with a letter or digit, so no choice has this value. */
export const UNMARKED = "-";

/** What the pickers and the task screen call the built-in area Main, whose tasks the main agent builds itself. */
export const MAIN_AREA_TEXT = "Main agent";

/** An area as the area functions read it: an area of the daemon's list, or the built-in area Main that `withMainArea` adds. */
export type AreaRef = Pick<AreaEntry, "name" | "label" | "lead"> & {
  main?: boolean;
};

/** An area as the pickers show it: the label that puts a task in it, and its lead; Main has no lead. */
export interface AreaOption {
  name: string;
  label: BoardLabel;
  lead: string | null;
  main: boolean;
}

/** The daemon's areas with Main first, over the label the daemon names for it; without that name, the areas alone. The daemon refuses an area named like that label, so the name marks Main in the menus. */
export function withMainArea(
  areas: readonly AreaEntry[],
  mainLabel: string | null,
): AreaRef[] {
  if (mainLabel === null) return [...areas];
  return [
    { name: mainLabel, label: mainLabel, lead: null, main: true },
    ...areas,
  ];
}

/** What an area row says beside its label: Main's owner, or the area's lead. */
export function ownerText(option: AreaOption): string {
  return option.main ? MAIN_AREA_TEXT : leadText(option.lead);
}

/** The line the task screen shows beside the label of the task's area. */
export function ownerLine(option: AreaOption): string {
  return option.main ? MAIN_AREA_TEXT : `lead: ${leadText(option.lead)}`;
}

/** What the human review group says on a task of Main: a human approves while the mark is on, and turning it off is the user's word that the main agent approves the proposal itself. Null for a task of another area. */
export function mainReviewNote(main: boolean, marked: boolean): string | null {
  if (!main) return null;
  return marked
    ? "A task of the main agent: a human approves its proposal. Turn off human review to let the main agent approve it."
    : "A task of the main agent: the main agent approves its own proposal.";
}

/** Whether the task's labels put it in Main. */
export function inMainArea(
  taskLabels: readonly BoardLabel[],
  areas: readonly AreaRef[],
): boolean {
  return areas.some(
    (area) =>
      area.main === true && taskLabels.some((l) => l.name === area.label),
  );
}

/** The project's label of that name; a label missing from the list is drawn in stone. */
export function labelNamed(
  labels: readonly BoardLabel[],
  name: string,
): BoardLabel {
  return labels.find((l) => l.name === name) ?? { name, color: "stone" };
}

/** Every area in the order of `areas`, with the colour of its label. */
export function areaOptions(
  areas: readonly AreaRef[],
  labels: readonly BoardLabel[],
): AreaOption[] {
  return areas.map((area) => ({
    name: area.name,
    label: labelNamed(labels, area.label),
    lead: area.lead,
    main: area.main === true,
  }));
}

/** An area has no lead until its first lead starts. */
export function leadText(lead: string | null): string {
  return lead ?? "no lead yet";
}

function areaLabelNames(areas: readonly AreaRef[]): Set<string> {
  return new Set(areas.map((area) => area.label));
}

/** The task's labels split into its areas, in area order, and its other labels. The daemon refuses a second area label, but a task can hold two from older data until a person picks one. */
export function taskLabelView(
  taskLabels: readonly BoardLabel[],
  areas: readonly AreaRef[],
): { areas: AreaOption[]; others: BoardLabel[] } {
  const areaNames = areaLabelNames(areas);
  return {
    areas: areaOptions(
      areas.filter((area) => taskLabels.some((l) => l.name === area.label)),
      taskLabels,
    ),
    others: taskLabels.filter((label) => !areaNames.has(label.name)),
  };
}

/** The value the area menu marks: the task's one area, or no area. A task with two areas marks nothing, also when the daemon flags the conflict with areas the client does not know yet. */
export function markedArea(
  view: { areas: readonly AreaOption[] },
  conflict: boolean,
): string {
  const [only, ...rest] = view.areas;
  if (conflict || rest.length > 0) {
    return UNMARKED;
  }
  return only?.name ?? NO_AREA;
}

/** The labels the labels field offers: labels that are no area's label and are not chosen yet, whose name contains the query. */
export function labelChoices(
  labels: readonly BoardLabel[],
  areas: readonly AreaRef[],
  chosen: readonly string[],
  query: string,
): BoardLabel[] {
  const areaNames = areaLabelNames(areas);
  const needle = query.trim().toLowerCase();
  return labels.filter(
    (label) =>
      !areaNames.has(label.name) &&
      !chosen.includes(label.name) &&
      label.name.toLowerCase().includes(needle),
  );
}

function areaLabelOf(
  areas: readonly AreaRef[],
  area: string,
): string | undefined {
  return areas.find((a) => a.name === area)?.label;
}

/** The label names a new task is created with: the chosen area's label, then the other labels. */
export function createLabels(
  areas: readonly AreaRef[],
  area: string,
  labels: readonly string[],
): string[] {
  const label = area === NO_AREA ? undefined : areaLabelOf(areas, area);
  const names = label === undefined ? [...labels] : [label, ...labels];
  return [...new Set(names)];
}

/** The task's whole label set after its area changes to `next`: every area label goes, the next area's label comes. */
export function swapArea(
  taskLabels: readonly BoardLabel[],
  areas: readonly AreaRef[],
  next: string,
): string[] {
  const areaNames = areaLabelNames(areas);
  const kept = taskLabels
    .map((label) => label.name)
    .filter((name) => !areaNames.has(name));
  const label = next === NO_AREA ? undefined : areaLabelOf(areas, next);
  return label === undefined ? kept : [...kept, label];
}

/** Whether picking `next` changes the task: a task with two areas changes for any pick. */
export function areaChanges(
  view: { areas: readonly AreaOption[] },
  conflict: boolean,
  next: string,
): boolean {
  return markedArea(view, conflict) !== next;
}

/**
 * Which label controls take input. The area write sends the whole label set, so a label added or removed while it is
 * in flight would be undone by it: every control waits while any label write runs. A task with two areas takes only the
 * area change, since the daemon refuses every other label change until one area is left.
 */
export function labelControls(
  conflict: boolean,
  writing: boolean,
): { area: boolean; labels: boolean } {
  return { area: !writing, labels: !writing && !conflict };
}
