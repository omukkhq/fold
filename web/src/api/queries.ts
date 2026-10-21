import { queryOptions } from "@tanstack/react-query";
import { getComparison, getFileDiff } from "./diff";
import { RECENT_EVENTS, listAgents } from "./agents";
import { getProjectGit } from "./git";
import { keys } from "./keys";
import { countAwaitingHuman, listThreads } from "./messages";
import { getHostNotify, getProjectNotify } from "./notify";
import { getProject, listProjects } from "./projects";
import {
  getProposal,
  getProposalChanges,
  getProposalFile,
  getProposalFileDiff,
  listComments,
  listProposals,
  type ProposalList,
  type ChangeRange,
  getCheckRun,
} from "./proposals";
import { getRequest, getThread, listRequests } from "./requests";
import { getReviewBatch, listReviewBatches } from "./reviews";
import { getSession } from "./session";
import { listNeedsYou, listWorkerQuestions, listWorkers } from "./workers";
import { getWorkspace } from "./workspaces";

export const projectsQuery = () =>
  queryOptions({ queryKey: keys.projects(), queryFn: listProjects });

export const projectQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.project(slug),
    queryFn: () => getProject(slug),
  });

export const gitQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.git(slug),
    queryFn: () => getProjectGit(slug),
  });

export const sessionQuery = () =>
  queryOptions({ queryKey: keys.session(), queryFn: getSession });

/** Recorded states never change; a workspace side is read again when the event stream reports a change. */
export const comparisonQuery = (
  slug: string,
  from: string | undefined,
  to: string,
) =>
  queryOptions({
    queryKey: keys.comparison(slug, from, to),
    queryFn: () => getComparison(slug, from, to),
    staleTime: Infinity,
  });

export const fileDiffQuery = (
  slug: string,
  from: string | undefined,
  to: string,
  path: string,
) =>
  queryOptions({
    queryKey: keys.fileDiff(slug, from, to, path),
    queryFn: () => getFileDiff(slug, from, to, path),
    staleTime: Infinity,
  });

/** The event stream keeps these fresh, so a cached answer never goes stale by time. */
export const agentsQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.agents(slug),
    queryFn: () => listAgents(slug, RECENT_EVENTS),
    staleTime: Infinity,
  });

export const messagesQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.messages(slug),
    queryFn: () => listThreads(slug),
    staleTime: Infinity,
  });

/** A thread the listing did not reach, read by the id of any of its messages. */
export const threadQuery = (slug: string, thread: string) =>
  queryOptions({
    queryKey: keys.messagesThread(slug, thread),
    queryFn: () => getThread(slug, thread),
    staleTime: Infinity,
  });

export const awaitingHumanQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.messagesAwaiting(slug),
    queryFn: () => countAwaitingHuman(slug),
    staleTime: Infinity,
  });

/** The proposal list and the proposals waiting for the signed-in human share one read, so the Proposals screen never fetches it twice. */
export const proposalsQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.proposals(slug),
    queryFn: () => listProposals(slug),
    staleTime: Infinity,
    select: (list: ProposalList) => list.proposals,
  });

/** The open proposals whose current version waits for the signed-in human's review, as the daemon decides them. */
export const proposalsWaitingQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.proposals(slug),
    queryFn: () => listProposals(slug),
    staleTime: Infinity,
    select: (list: ProposalList) => list.waiting_for_you,
  });

export const proposalQuery = (slug: string, id: string) =>
  queryOptions({
    queryKey: keys.proposal(slug, id),
    queryFn: () => getProposal(slug, id),
    staleTime: Infinity,
  });

/** A check run with its logs; the event stream refreshes it when the run ends. */
export const checkRunQuery = (slug: string, run: string) =>
  queryOptions({
    queryKey: keys.checkRun(slug, run),
    queryFn: () => getCheckRun(slug, run),
    staleTime: Infinity,
  });

export const proposalChangesQuery = (
  slug: string,
  id: string,
  range: ChangeRange,
) =>
  queryOptions({
    queryKey: keys.proposalChanges(slug, id, range.version, range.since),
    queryFn: () => getProposalChanges(slug, id, range),
    staleTime: Infinity,
  });

export const proposalFileDiffQuery = (
  slug: string,
  id: string,
  range: ChangeRange,
  path: string,
) =>
  queryOptions({
    queryKey: keys.proposalFileDiff(slug, id, range.version, range.since, path),
    queryFn: () => getProposalFileDiff(slug, id, range, path),
    staleTime: Infinity,
  });

export const proposalFileQuery = (
  slug: string,
  id: string,
  version: number,
  path: string,
) =>
  queryOptions({
    queryKey: keys.proposalFile(slug, id, version, path),
    queryFn: () => getProposalFile(slug, id, version, path),
    staleTime: Infinity,
  });

export const workspaceQuery = (slug: string, id: string) =>
  queryOptions({
    queryKey: keys.workspace(slug, id),
    queryFn: () => getWorkspace(slug, id),
    staleTime: Infinity,
  });

export const proposalCommentsQuery = (slug: string, id: string) =>
  queryOptions({
    queryKey: keys.proposalComments(slug, id),
    queryFn: () => listComments(slug, id),
    staleTime: Infinity,
  });

export const requestsQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.requests(slug),
    queryFn: () => listRequests(slug),
    staleTime: Infinity,
  });

export const requestQuery = (slug: string, id: string) =>
  queryOptions({
    queryKey: keys.request(slug, id),
    queryFn: () => getRequest(slug, id),
    staleTime: Infinity,
  });

export const requestThreadQuery = (slug: string, id: string, thread: string) =>
  queryOptions({
    queryKey: keys.requestThread(slug, id, thread),
    queryFn: () => getThread(slug, thread),
    staleTime: Infinity,
  });

export const workersQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.workers(slug),
    queryFn: () => listWorkers(slug),
    staleTime: Infinity,
  });

export const workerQuestionsQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.workerQuestions(slug),
    queryFn: () => listWorkerQuestions(slug),
    staleTime: Infinity,
  });

export const needsYouQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.needsYou(slug),
    queryFn: () => listNeedsYou(slug),
    staleTime: Infinity,
  });

export const reviewBatchesQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.reviewBatches(slug),
    queryFn: () => listReviewBatches(slug),
    staleTime: Infinity,
  });

export const reviewBatchQuery = (slug: string, id: string) =>
  queryOptions({
    queryKey: keys.reviewBatch(slug, id),
    queryFn: () => getReviewBatch(slug, id),
    staleTime: Infinity,
  });

export const projectNotifyQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.notify(slug),
    queryFn: () => getProjectNotify(slug),
  });

export const hostNotifyQuery = () =>
  queryOptions({ queryKey: keys.hostNotify(), queryFn: getHostNotify });
