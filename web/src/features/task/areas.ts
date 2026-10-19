import type { BoardLabel } from "@/api/board";
import type { AreaEntry } from "@/api/workers";

/** The area field's value when the task gets no area label. */
export const NO_AREA = "";

export const NO_AREA_TEXT = "Let the main agent decide";

/** The area menu's value when it marks no choice. An area name starts with a letter or digit, so no choice has this value. */
export const UNMARKED = "-";

/** An area as the pickers show it: the label that puts a task in it, and its lead. */
export interface AreaOption {
  name: string;
  label: BoardLabel;
  lead: string | null;
}

/** The project's label of that name; a label missing from the list is drawn in stone. */
export function labelNamed(
  labels: readonly BoardLabel[],
  name: string,
): BoardLabel {
  return labels.find((l) => l.name === name) ?? { name, color: "stone" };
}

/** Every area in the daemon's name order, with the colour of its label. */
export function areaOptions(
  areas: readonly AreaEntry[],
  labels: readonly BoardLabel[],
): AreaOption[] {
  return areas.map((area) => ({
    name: area.name,
    label: labelNamed(labels, area.label),
    lead: area.lead,
  }));
}

/** An area has no lead until its first lead starts. */
export function leadText(lead: string | null): string {
  return lead ?? "no lead yet";
}

function areaLabelNames(areas: readonly AreaEntry[]): Set<string> {
  return new Set(areas.map((area) => area.label));
}

/** The task's labels split into its areas, in area order, and its other labels. The daemon refuses a second area label, but a task can hold two from older data until a person picks one. */
export function taskLabelView(
  taskLabels: readonly BoardLabel[],
  areas: readonly AreaEntry[],
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
  areas: readonly AreaEntry[],
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
  areas: readonly AreaEntry[],
  area: string,
): string | undefined {
  return areas.find((a) => a.name === area)?.label;
}

/** The label names a new task is created with: the chosen area's label, then the other labels. */
export function createLabels(
  areas: readonly AreaEntry[],
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
  areas: readonly AreaEntry[],
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
