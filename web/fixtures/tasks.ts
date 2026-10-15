import type { BoardCard, BoardLabel, TaskStatus } from "../src/api/board";
import { ApiError } from "../src/api/errors";
import type {
  CheckpointSummary,
  FeedbackRecipient,
  LiveWorkspace,
  MessageTargets,
  NewChecklistItem,
  TaskChangeset,
  TaskClaim,
  TaskDetail,
  TaskEdge,
  TaskEvent,
  TaskHumanReview,
  TaskIntegration,
  TaskMessage,
  TaskProposal,
  TaskWorkspace,
} from "../src/api/tasks";
import { demoCards } from "./demoBoard";
import { session } from "./session";

const SECOND = 1_000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;

type StoredItem =
  | { id: string; kind: "step"; text: string; done: boolean }
  | { id: string; kind: "linked"; task: string };

interface StoredChecklist {
  name: string;
  items: StoredItem[];
}

interface StoredEdge {
  needs: string;
  origin: "direct" | "checklist";
}

/** The store keeps edges one way and items without their child's title; reads derive both, as the daemon does. */
interface StoredTask {
  task: string;
  title: string;
  description: string;
  status: TaskStatus;
  status_reason: string;
  blocked: boolean;
  labels: BoardLabel[];
  checklists: StoredChecklist[];
  needs: StoredEdge[];
  claims: TaskClaim[];
  activity: TaskDetail["activity"];
  attention: TaskDetail["attention"];
  workspace: TaskWorkspace | null;
  checkpoints: CheckpointSummary[];
  changesets: TaskChangeset[];
  proposal: TaskProposal | null;
  integration: TaskIntegration | null;
  messages: TaskMessage[];
  human_review: TaskHumanReview | null;
  can_change_human_review: boolean;
  history: TaskEvent[];
}

const DESCRIPTION = `The sign-in screen: email and password with inline validation, a pending state while the request runs, and the server's error codes shown as plain messages. The form posts to \`/session\`.

**Scope**

- Fields, validation schema and submit handling in \`src/routes/login.tsx\`
- Messages for \`invalid_credentials\` and \`rate_limited\`
- No sign-up or password reset (T-25)

**Acceptance**

- vitest covers the schema and the error mapping; the form never shows success before the server answers

| Code | Message |
| --- | --- |
| \`invalid_credentials\` | The email or the password is wrong. |
| \`rate_limited\` | Too many attempts. Try again later. |

- [x] Agree the error copy
- [ ] Check the copy with product`;

/** The default checklist items of a task that has only counts on the board. */
function steps(card: BoardCard, prefix: string): StoredItem[] {
  return Array.from({ length: card.checklist.steps_total }, (_, i) => ({
    id: `${prefix}-${String(i + 1)}`,
    kind: "step" as const,
    text: `Step ${String(i + 1)} of ${card.title.toLowerCase()}`,
    done: i < card.checklist.steps_done,
  }));
}

const REASONS: Record<TaskStatus, string> = {
  backlog: "Calculated: waiting for its prerequisites",
  todo: "Calculated: every prerequisite is done, nobody has started",
  in_progress: "Calculated: the workspace is open, no proposal yet",
  review: "Calculated: a proposal is waiting for review",
  done: "Calculated: the proposal was integrated",
};

/** The board's dependency edges, as "task needs prerequisite". */
const EDGES: [string, string][] = [
  ["T-5", "T-2"],
  ["T-5", "T-4"],
  ["T-6", "T-4"],
  ["T-7", "T-4"],
  ["T-10", "T-3"],
  ["T-4", "T-14"],
  ["T-4", "T-15"],
  ["T-1", "T-15"],
];

function generic(card: BoardCard, now: number): StoredTask {
  const workspace: TaskWorkspace | null = card.workspace
    ? {
        id: card.workspace,
        directory: `ws/${card.task}-${(card.worker ?? "worker").replace("/", "-")}`,
        base: "r8",
        main: "r8",
        behind: 0,
        live: null,
      }
    : null;
  const claims: TaskClaim[] = card.worker
    ? [
        {
          kind: "implementation",
          holder: card.worker,
          claimed_at: now - 2 * HOUR,
          renewed_at: now - (card.attention.length > 0 ? 47 : 1) * MINUTE,
          stale: card.attention.some((a) => a.kind === "stale_claim"),
        },
      ]
    : [];
  return {
    task: card.task,
    title: card.title,
    description: `${card.title}.\n\n**Scope**\n\n- The work the title names, and nothing more`,
    status: card.status,
    status_reason: REASONS[card.status],
    blocked: card.blocked,
    labels: card.labels,
    checklists:
      card.checklist.steps_total > 0
        ? [{ name: "Subtasks", items: steps(card, card.task) }]
        : [],
    needs: EDGES.filter(([task]) => task === card.task).map(([, needs]) => ({
      needs,
      origin: "direct",
    })),
    claims,
    activity: card.activity,
    attention: card.attention,
    workspace,
    checkpoints: [],
    changesets: [],
    proposal: card.proposal
      ? {
          id: card.proposal.id,
          state: card.proposal.state,
          version: 1,
          reviews: [],
        }
      : null,
    integration:
      card.proposal?.revision != null
        ? {
            revision: card.proposal.revision,
            proposal: card.proposal.id,
            by: "review",
            at: now - 26 * HOUR,
          }
        : null,
    messages: [],
    human_review: card.human_review
      ? {
          reviewer: card.human_review.reviewer,
          reason: "Tasks labelled auth need my review",
          set_by: "planner",
          set_at: now - 3 * HOUR,
        }
      : null,
    can_change_human_review: true,
    history: [{ type: "task.created", actor: "planner", at: now - 5 * HOUR }],
  };
}

function loginForm(card: BoardCard, now: number): StoredTask {
  const live: LiveWorkspace = {
    workspace: "ws-2",
    task: "T-2",
    worker: "frontend/ui",
    base: "r7",
    files: [
      ["src/routes/login.tsx", "modified", 42, 8],
      ["src/forms/login-schema.ts", "added", 31, 0],
      ["src/api/auth.ts", "modified", 6, 2],
      ["src/routes/old-login.tsx", "deleted", 0, 57],
    ].map(([path, change, added, removed]) => ({
      path: String(path),
      previous_path: null,
      change: change as LiveWorkspace["files"][number]["change"],
      added_lines: Number(added),
      removed_lines: Number(removed),
      binary: false,
    })),
    summary: "4 files, +79 -67",
    last_activity_at: now - 12 * SECOND,
    checkpoints: [],
    proposal: null,
  };
  return {
    ...generic(card, now),
    description: DESCRIPTION,
    status_reason: "Calculated: workspace ws-2 is open, no proposal yet",
    checklists: [
      {
        name: "Subtasks",
        items: [
          {
            id: "T-2-1",
            kind: "step",
            text: "Email and password fields with inline errors",
            done: true,
          },
          {
            id: "T-2-2",
            kind: "step",
            text: "Validation schema with zod",
            done: true,
          },
          {
            id: "T-2-3",
            kind: "step",
            text: "Disable submit while the request is pending",
            done: true,
          },
          {
            id: "T-2-4",
            kind: "step",
            text: "Map invalid_credentials and rate_limited to messages",
            done: false,
          },
          { id: "T-2-5", kind: "linked", task: "T-13" },
        ],
      },
    ],
    needs: [{ needs: "T-13", origin: "checklist" }],
    claims: [
      {
        kind: "implementation",
        holder: "frontend/ui",
        claimed_at: now - 40 * MINUTE,
        renewed_at: now - 2 * MINUTE,
        stale: false,
      },
    ],
    workspace: {
      id: "ws-2",
      directory: "ws/T-2-frontend-ui",
      base: "r7",
      main: "r8",
      behind: 1,
      live,
    },
    checkpoints: [
      {
        id: "ck-31",
        label: "quiet 2 min",
        automatic: true,
        created_at: now - 3 * MINUTE,
      },
      {
        id: "ck-30",
        label: "before the schema",
        automatic: false,
        created_at: now - 18 * MINUTE,
      },
      {
        id: "ck-29",
        label: null,
        automatic: true,
        created_at: now - 31 * MINUTE,
      },
    ],
    changesets: [
      {
        id: "C-18",
        message: "Form fields and validation",
        created_at: now - 25 * MINUTE,
      },
    ],
    messages: [
      {
        id: "m-41",
        thread: "m-41",
        from: "frontend/ui",
        to: "human:tanim",
        body: "Question for you: should the rate-limit message say how long to wait, or stay generic?",
        at: now - 15 * MINUTE,
      },
      {
        id: "m-42",
        thread: "m-41",
        from: "human:tanim",
        to: "frontend/ui",
        body: "Keep the error text short and neutral. Product copy will review it later.",
        at: now - 4 * MINUTE,
      },
    ],
    history: [
      {
        type: "checkpoint.recorded",
        actor: "frontend/ui",
        at: now - 3 * MINUTE,
      },
      {
        type: "changeset.recorded",
        actor: "frontend/ui",
        at: now - 25 * MINUTE,
      },
      {
        type: "workspace.created",
        actor: "frontend/ui",
        at: now - 39 * MINUTE,
      },
      { type: "claim.acquired", actor: "frontend/ui", at: now - 40 * MINUTE },
      { type: "task.created", actor: "human:tanim", at: now - 2 * HOUR },
    ],
  };
}

const stores = new Map<string, Map<string, StoredTask>>();
const created = new Map<string, string[]>();

function tasks(slug: string): Map<string, StoredTask> {
  let store = stores.get(slug);
  if (store === undefined) {
    const now = Date.now();
    const cards = slug === "demo" ? demoCards(now) : [];
    store = new Map(
      cards.map((card) => [
        card.task,
        card.task === "T-2" ? loginForm(card, now) : generic(card, now),
      ]),
    );
    stores.set(slug, store);
  }
  return store;
}

/** Puts every project back to its first state, so each test starts from the same tasks. */
export function resetTasks(): void {
  stores.clear();
  created.clear();
}

function cardOf(found: StoredTask): BoardCard {
  const steps = found.checklists.flatMap((c) => c.items);
  const plain = steps.filter((i) => i.kind === "step");
  const linked = steps.filter((i) => i.kind === "linked");
  return {
    task: found.task,
    title: found.title,
    status: found.status,
    blocked: found.blocked,
    labels: found.labels,
    checklist: {
      steps_done: plain.filter((i) => i.done).length,
      steps_total: plain.length,
      linked_done: 0,
      linked_total: linked.length,
    },
    needs: found.needs.map((edge) => edge.needs),
    blocks: 0,
    worker: null,
    workspace: null,
    activity: null,
    proposal: null,
    human_review: found.human_review
      ? { reviewer: found.human_review.reviewer }
      : null,
    attention: [],
  };
}

/** The board fixture shows the tasks a human created in the dialog, as the daemon's board would. */
export function createdCards(slug: string): BoardCard[] {
  const store = stores.get(slug);
  return (created.get(slug) ?? []).flatMap((id) => {
    const found = store?.get(id);
    return found ? [cardOf(found)] : [];
  });
}

/** The signed-in human as an actor; the fixture session always has a name. */
function human(): string {
  return `human:${session.human ?? "tanim"}`;
}

function notFound(task: string): ApiError {
  return new ApiError("task_not_found", `task ${task} does not exist`);
}

function stored(slug: string, task: string): StoredTask {
  const found = tasks(slug).get(task);
  if (!found) {
    throw notFound(task);
  }
  return found;
}

function invalid(reason: string, message: string): ApiError {
  return new ApiError("invalid_request", message, { reason });
}

function edgeOf(
  all: Map<string, StoredTask>,
  other: string,
  direction: TaskEdge["direction"],
  origin: TaskEdge["origin"],
): TaskEdge {
  const found = all.get(other);
  return {
    task: other,
    title: found?.title ?? other,
    status: found?.status ?? "backlog",
    direction,
    origin,
  };
}

function read(slug: string, found: StoredTask): TaskDetail {
  const all = tasks(slug);
  const { needs, ...rest } = found;
  const neededBy = [...all.values()].flatMap((other) =>
    other.needs
      .filter((edge) => edge.needs === found.task)
      .map((edge) => edgeOf(all, other.task, "needed_by", edge.origin)),
  );
  return {
    ...rest,
    checklists: found.checklists.map((checklist) => ({
      name: checklist.name,
      items: checklist.items.map((item) => {
        if (item.kind === "step") {
          return item;
        }
        const child = all.get(item.task);
        const status = child?.status ?? "backlog";
        return {
          id: item.id,
          kind: "linked" as const,
          task: item.task,
          title: child?.title ?? item.task,
          status,
          done: status === "done",
        };
      }),
    })),
    dependencies: [
      ...needs.map((edge) => edgeOf(all, edge.needs, "needs", edge.origin)),
      ...neededBy,
    ],
  };
}

export function task(slug: string, id: string): TaskDetail | null {
  const found = tasks(slug).get(id);
  return found ? read(slug, found) : null;
}

export function messageTargets(slug: string): MessageTargets {
  if (slug !== "demo") {
    return { agents: [], tags: [] };
  }
  return {
    agents: ["frontend", "backend", "review"],
    tags: ["frontend", "backend", "integrator"],
  };
}

function record(found: StoredTask, type: TaskEvent["type"]): void {
  found.history.unshift({
    type,
    actor: human(),
    at: Date.now(),
  });
}

export function setTitle(slug: string, id: string, title: string): void {
  const found = stored(slug, id);
  found.title = title;
  record(found, "task.updated");
}

export function setDescription(
  slug: string,
  id: string,
  description: string,
): void {
  const found = stored(slug, id);
  found.description = description;
  record(found, "task.updated");
}

/** A name the project already has keeps the project's colour, as labels are project-wide. */
export function addLabel(slug: string, id: string, label: BoardLabel): void {
  const found = stored(slug, id);
  const known = [...tasks(slug).values()]
    .flatMap((t) => t.labels)
    .find((l) => l.name === label.name);
  if (found.labels.some((l) => l.name === label.name)) {
    return;
  }
  found.labels = [...found.labels, known ?? label];
  record(found, "task.updated");
}

export function removeLabel(slug: string, id: string, name: string): void {
  const found = stored(slug, id);
  found.labels = found.labels.filter((l) => l.name !== name);
  record(found, "task.updated");
}

/** The path from `from` to `to` along "needs" edges, or null when there is none. */
function pathBetween(slug: string, from: string, to: string): string[] | null {
  const all = tasks(slug);
  const seen = new Set<string>();
  const walk = (at: string): string[] | null => {
    if (at === to) {
      return [at];
    }
    if (seen.has(at)) {
      return null;
    }
    seen.add(at);
    for (const edge of all.get(at)?.needs ?? []) {
      const rest = walk(edge.needs);
      if (rest) {
        return [at, ...rest];
      }
    }
    return null;
  };
  return walk(from);
}

function addEdge(
  slug: string,
  found: StoredTask,
  needs: string,
  origin: StoredEdge["origin"],
): void {
  if (needs === found.task) {
    throw invalid("self_dependency", "a task cannot depend on itself");
  }
  if (!tasks(slug).has(needs)) {
    throw notFound(needs);
  }
  const back = pathBetween(slug, needs, found.task);
  if (back) {
    const cycle = [found.task, ...back];
    throw new ApiError(
      "dependency_cycle",
      `the edge would close the cycle ${cycle.join(" → ")}`,
      { cycle },
    );
  }
  if (found.needs.some((edge) => edge.needs === needs)) {
    throw new ApiError(
      "dependency_exists",
      `${found.task} already needs ${needs}`,
    );
  }
  found.needs.push({ needs, origin });
}

function findItem(
  found: StoredTask,
  item: string,
): { list: StoredChecklist; index: number; value: StoredItem } {
  for (const list of found.checklists) {
    const index = list.items.findIndex((i) => i.id === item);
    const value = list.items[index];
    if (value) {
      return { list, index, value };
    }
  }
  throw new ApiError("not_found", `item ${item} does not exist`);
}

let nextItem = 100;

export function addChecklistItem(
  slug: string,
  id: string,
  checklist: string,
  item: NewChecklistItem,
): void {
  const found = stored(slug, id);
  if (item.kind === "linked") {
    addEdge(slug, found, item.task, "checklist");
  }
  let list = found.checklists.find((c) => c.name === checklist);
  if (!list) {
    list = { name: checklist, items: [] };
    found.checklists.push(list);
  }
  nextItem += 1;
  const itemId = `${id}-${String(nextItem)}`;
  list.items.push(
    item.kind === "step"
      ? { id: itemId, kind: "step", text: item.text, done: false }
      : { id: itemId, kind: "linked", task: item.task },
  );
  record(found, "checklist.item_added");
}

export function renameChecklistItem(
  slug: string,
  id: string,
  item: string,
  text: string,
): void {
  const found = stored(slug, id);
  const { value } = findItem(found, item);
  if (value.kind === "linked") {
    throw invalid("linked_item", "a linked item takes its child's title");
  }
  value.text = text;
  record(found, "checklist.item_updated");
}

export function moveChecklistItem(
  slug: string,
  id: string,
  item: string,
  position: number,
): void {
  const found = stored(slug, id);
  const { list, index, value } = findItem(found, item);
  const items = list.items.filter((_, i) => i !== index);
  const to = Math.max(0, Math.min(position, items.length));
  list.items = [...items.slice(0, to), value, ...items.slice(to)];
  record(found, "checklist.item_updated");
}

export function removeChecklistItem(
  slug: string,
  id: string,
  item: string,
): void {
  const found = stored(slug, id);
  const { list, index, value } = findItem(found, item);
  list.items = list.items.filter((_, i) => i !== index);
  if (value.kind === "linked") {
    found.needs = found.needs.filter(
      (edge) => !(edge.needs === value.task && edge.origin === "checklist"),
    );
  }
  record(found, "checklist.item_removed");
}

export function tickChecklistItem(
  slug: string,
  id: string,
  item: string,
  done: boolean,
): void {
  const found = stored(slug, id);
  const { value } = findItem(found, item);
  if (value.kind === "linked") {
    throw invalid(
      "linked_item",
      "fold ticks a linked item when its child is done",
    );
  }
  value.done = done;
  record(found, "checklist.item_updated");
}

export function addDependency(slug: string, id: string, needs: string): void {
  const found = stored(slug, id);
  if (found.status === "done") {
    throw new ApiError("task_closed", `${id} is done`);
  }
  addEdge(slug, found, needs, "direct");
  record(found, "dependency.added");
}

export function removeDependency(
  slug: string,
  id: string,
  needs: string,
): void {
  const found = stored(slug, id);
  const edge = found.needs.find((e) => e.needs === needs);
  if (!edge) {
    throw new ApiError("dependency_not_found", `${id} does not need ${needs}`);
  }
  if (edge.origin === "checklist") {
    throw invalid(
      "checklist_origin",
      "the edge belongs to a linked item; remove the item instead",
    );
  }
  found.needs = found.needs.filter((e) => e.needs !== needs);
  record(found, "dependency.removed");
}

export function setHumanReview(
  slug: string,
  id: string,
  mark: { reviewer: string | null; reason: string },
): void {
  const found = stored(slug, id);
  found.human_review = {
    ...mark,
    set_by: human(),
    set_at: Date.now(),
  };
  record(found, "task.human_review_set");
}

export function clearHumanReview(slug: string, id: string): void {
  const found = stored(slug, id);
  found.human_review = null;
  record(found, "task.human_review_cleared");
}

function recipient(found: StoredTask, to: FeedbackRecipient): string {
  switch (to.kind) {
    case "holder": {
      const holder = found.claims.find((c) => c.kind === "implementation");
      if (!holder) {
        throw new ApiError("invalid_request", "nobody holds the task");
      }
      return holder.holder;
    }
    case "agent":
      return to.name;
    case "tag":
      return `tag:${to.name}`;
  }
}

let nextMessage = 300;

export function sendFeedback(
  slug: string,
  id: string,
  feedback: { body: string; to: FeedbackRecipient },
): void {
  const found = stored(slug, id);
  nextMessage += 1;
  const messageId = `m-${String(nextMessage)}`;
  found.messages.push({
    id: messageId,
    thread: messageId,
    from: human(),
    to: recipient(found, feedback.to),
    body: feedback.body,
    at: Date.now(),
  });
}

/** A new task has no edges and nobody working on it, so the daemon calculates Todo. */
export function createTask(
  slug: string,
  input: { title: string; description: string },
): string {
  const all = tasks(slug);
  const number =
    Math.max(0, ...[...all.keys()].map((id) => Number(id.slice(2)))) + 1;
  const id = `T-${String(number)}`;
  const now = Date.now();
  all.set(id, {
    task: id,
    title: input.title,
    description: input.description,
    status: "todo",
    status_reason: REASONS.todo,
    blocked: false,
    labels: [],
    checklists: [],
    needs: [],
    claims: [],
    activity: null,
    attention: [],
    workspace: null,
    checkpoints: [],
    changesets: [],
    proposal: null,
    integration: null,
    messages: [],
    human_review: null,
    can_change_human_review: true,
    history: [{ type: "task.created", actor: human(), at: now }],
  });
  created.set(slug, [...(created.get(slug) ?? []), id]);
  return id;
}
