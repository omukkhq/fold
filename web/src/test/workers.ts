import type { ReviewBatch, ReviewBatchMember } from "@/api/reviews";
import type { ProposalSummary } from "@/api/proposals";
import type {
  AreaEntry,
  NeedsYouItem,
  WorkerList,
  WorkerProcess,
  WorkerQuestion,
} from "@/api/workers";

export function workerProcess(
  name: string,
  fields: Partial<WorkerProcess> = {},
): WorkerProcess {
  return {
    name,
    role: "area_lead",
    area: "web",
    state: "working",
    model: "opus",
    tasks: [],
    proposals: [],
    started_by: "human:tanim",
    started_at: 1_760_000_000_000,
    stopped_at: null,
    turns: 61,
    cost_micro_usd: 5_870_000,
    cache_read_tokens: 910,
    cache_write_tokens: 90,
    context_tokens: 164_000,
    exit_code: null,
    failed_reason: null,
    last_error: null,
    last_error_at: null,
    failed_turns: 0,
    model_unreachable: false,
    hint: null,
    cache_ttl: "1h",
    session_id: "0f8e2c1a-5b7d-4c3e-9a1f-2d4b6c8e0a13",
    previous_sessions: [],
    transcript:
      "/home/tanim/.claude/projects/-home-tanim-ledger/0f8e2c1a-5b7d-4c3e-9a1f-2d4b6c8e0a13.jsonl",
    ...fields,
  };
}

/** An area without a lead or a profile. */
export function areaEntry(
  name: string,
  label: string,
  fields: Partial<AreaEntry> = {},
): AreaEntry {
  return {
    name,
    label,
    lead: null,
    instructions: null,
    model: null,
    mcp: null,
    ...fields,
  };
}

export function workerList(fields: Partial<WorkerList> = {}): WorkerList {
  return {
    areas: [],
    main_label: "main",
    workers: [],
    roles: [],
    limits: null,
    pause: { paused_since: null, paused_by: null },
    main_supervised: false,
    ...fields,
  };
}

export function permissionQuestion(
  fields: Partial<WorkerQuestion> = {},
): WorkerQuestion {
  return {
    id: "pq-3",
    worker: "web-worker",
    tool: "Bash",
    input: {},
    asked_at: 1_760_000_000_000,
    answer: null,
    ...fields,
  };
}

export function needsItem(fields: Partial<NeedsYouItem> = {}): NeedsYouItem {
  return {
    kind: "question",
    screen: "messages",
    since: 1_760_000_000_000,
    message: null,
    thread: null,
    question: null,
    agent: null,
    tool: null,
    task: null,
    proposal: null,
    request: null,
    human: null,
    humans: [],
    choices: [],
    header: null,
    title: null,
    reason: null,
    commit: null,
    paths: [],
    ...fields,
  };
}

export function member(
  proposal: string,
  fields: Partial<ReviewBatchMember> = {},
): ReviewBatchMember {
  return {
    proposal,
    task: "T-12",
    position: 1,
    version: 1,
    current_version: 1,
    in_batch: true,
    since: null,
    proposal_state: "open",
    ...fields,
  };
}

export function batch(fields: Partial<ReviewBatch> = {}): ReviewBatch {
  return {
    id: "rb-3",
    holder: "review-b3",
    state: "open",
    created_at: 1_760_000_000_000,
    submitted_at: null,
    closed_at: null,
    checkout: null,
    comments: [],
    members: [],
    verdicts: [],
    ...fields,
  };
}

export function proposalSummary(
  id: string,
  fields: Partial<ProposalSummary> = {},
): ProposalSummary {
  return {
    id,
    task: "T-12",
    title: `Proposal ${id}`,
    author: "web-worker",
    state: "open",
    current_version: 1,
    latest_verdict: null,
    review_claim: null,
    turn: "with_reviewer",
    conflicted: false,
    checks_failed: false,
    human_review: null,
    human_approved: true,
    close_reason: null,
    closed_at: null,
    created_at: 1_760_000_000_000,
    updated_at: 1_760_000_000_000,
    integration_queued: false,
    ...fields,
  };
}
