import type { Int64, Keyed, Node, Time } from './db';

// The shapes of what the server's view database holds (server/src/view.rs), as plain types: each
// message's fields as protocol/gem.proto has them, under pbjson's camelCase names, read with
// `Node`'s getters. Enums are their names.

export type MergeBy = 'MERGE_BY_UNSPECIFIED' | 'MERGE_BY_APPROVAL' | 'MERGE_BY_ADMIN';
export type AgentState = 'AGENT_STATE_WORKING' | 'AGENT_STATE_QUEUED' | 'AGENT_STATE_WAITING';

// A moment of the view.
export interface Moment {
  // Each cluster by its key, the URL of its oldest pull request.
  clusters: Keyed<Cluster>;
  // The clusters' keys, most recently updated first.
  order: string[];
  login: string;
  search: string;
  // The agents acting on no open pull request.
  agents: Agent[];
}

export interface Cluster {
  // Oldest first.
  pullRequests: PullRequest[];
  updatedAt: Time;
  name: string;
  mergeBy: MergeBy;
  agents: Agent[];
}

export interface PullRequest {
  repository: Repository;
  number: number;
  title: string;
  url: string;
  approved: boolean;
  checksPassing: boolean;
  checksFailing: boolean;
  proposals: Proposal[];
  behind: boolean;
  checks: Checks;
  headOid: string;
  mergeBy: MergeBy;
  files: FileDiff[];
  base: string;
  conflicts: boolean;
  rebasing: boolean;
  rebased: boolean;
  merging: boolean;
  noProposalReason: string;
  createdAt: Time;
}

export interface Checks {
  total: number;
  done: number;
  failed: number;
}

export interface Repository {
  owner: string;
  name: string;
}

export interface Agent {
  id: string;
  title: string;
  state: AgentState;
  pullRequestUrls: string[];
  reply: string;
  replyHtml: string;
  proposals: Proposal[];
  startedAt: Time;
}

export interface Proposal {
  id: string;
  summary: string;
  pullRequestUrl: string;
  comment: Comment;
  threadId: string;
  patch: Patch;
  reply: string;
  resolveThread: boolean;
  error: string;
  requestedTool: string;
  earlierComments: Comment[];
  replyHtml: string;
  followUp: string;
  followUpHtml: string;
  closePullRequest: boolean;
  checkFailure: CheckFailure;
  rerun: boolean;
  summaryHtml: string;
  investigationId: string;
  otherPatches: Patch[];
  openPullRequest: NewPullRequest;
}

export interface NewPullRequest {
  repository: string;
  base: string;
  branch: string;
  title: string;
  body: string;
  bodyHtml: string;
  patch: Patch;
}

export interface CheckFailure {
  id: string;
  headOid: string;
  checks: FailedCheck[];
}

export interface FailedCheck {
  id: string;
  name: string;
  conclusion: string;
  url: string;
  workflowRunId: Int64;
  excerpt: string;
}

export interface Comment {
  id: string;
  author: string;
  body: string;
  url: string;
  bodyHtml: string;
  path: string;
  line: number;
  diffHunk: string;
}

export interface FileDiff {
  path: string;
  patch: string;
}

export interface Patch {
  diff: string;
  commitMessage: string;
  headOid: string;
  pullRequestUrl: string;
}

// The enums, with the value each holds when it's left out.
export const mergeBy = (node: Node<Cluster> | Node<PullRequest>): MergeBy =>
  (node as Node<Cluster>).enum('mergeBy', 'MERGE_BY_UNSPECIFIED');
export const agentState = (agent: Node<Agent>): AgentState => agent.enum('state', 'AGENT_STATE_WORKING');

// A cluster's key, which links to it: the URL of its oldest pull request.
export const clusterKey = (cluster: Node<Cluster>): string => cluster.list('pullRequests')[0]?.str('url') ?? '';

// What the UI shows of a moment: the clusters in order, and the rest.
export function view(moment: Node<Moment> | undefined) {
  if (!moment) return { clusters: [], agents: [], login: '', search: '' };
  const clusters = moment.node('clusters');
  return {
    clusters: moment.strings('order').flatMap((key) => clusters?.at(key) ?? []),
    agents: moment.list('agents'),
    login: moment.str('login'),
    search: moment.str('search'),
  };
}
