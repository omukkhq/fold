import { queryOptions } from "@tanstack/react-query";
import { getComparison, getFileDiff } from "./diff";
import { RECENT_EVENTS, listAgents } from "./agents";
import { keys } from "./keys";
import { countAwaitingHuman, listThreads } from "./messages";
import { getProject, listProjects } from "./projects";
import { getProposal, listComments, listProposals } from "./proposals";
import { getRequest, getThread, listRequests } from "./requests";
import { getSession } from "./session";
import { getWorkspace } from "./workspaces";

export const projectsQuery = () =>
  queryOptions({ queryKey: keys.projects(), queryFn: listProjects });

export const projectQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.project(slug),
    queryFn: () => getProject(slug),
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

export const awaitingHumanQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.messagesAwaiting(slug),
    queryFn: () => countAwaitingHuman(slug),
    staleTime: Infinity,
  });

export const proposalsQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.proposals(slug),
    queryFn: () => listProposals(slug),
    staleTime: Infinity,
  });

export const proposalQuery = (slug: string, id: string) =>
  queryOptions({
    queryKey: keys.proposal(slug, id),
    queryFn: () => getProposal(slug, id),
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
