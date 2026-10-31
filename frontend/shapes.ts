import { bool, enumOf, type Infer, int64, keyed, list, num, optional, shape, str, time } from './db';

// The shapes of what the server's view database holds (server/src/view.rs): each message's fields
// as protocol/gem.proto has them, in its order, under pbjson's camelCase names. Each `shape` is
// both the message's type, through `Infer`, and how it's read and written. Enums are their names.

export type MergeBy = 'MERGE_BY_UNSPECIFIED' | 'MERGE_BY_APPROVAL' | 'MERGE_BY_ADMIN';
export type AgentState = 'AGENT_STATE_WORKING' | 'AGENT_STATE_QUEUED' | 'AGENT_STATE_WAITING';

export const Repository = shape({
  owner: str,
  name: str,
});
export type Repository = Infer<typeof Repository>;

export const Checks = shape({
  total: num,
  done: num,
  failed: num,
});
export type Checks = Infer<typeof Checks>;

export const Comment = shape({
  id: str,
  author: str,
  body: str,
  url: str,
  bodyHtml: str,
  path: str,
  line: num,
  diffHunk: str,
});
export type Comment = Infer<typeof Comment>;

export const FileDiff = shape({
  path: str,
  patch: str,
});
export type FileDiff = Infer<typeof FileDiff>;

export const Patch = shape({
  diff: str,
  commitMessage: str,
  headOid: str,
  pullRequestUrl: str,
});
export type Patch = Infer<typeof Patch>;

export const FailedCheck = shape({
  id: str,
  name: str,
  conclusion: str,
  url: str,
  workflowRunId: int64,
  excerpt: str,
});
export type FailedCheck = Infer<typeof FailedCheck>;

export const CheckFailure = shape({
  id: str,
  headOid: str,
  checks: list(FailedCheck),
});
export type CheckFailure = Infer<typeof CheckFailure>;

export const NewPullRequest = shape({
  repository: str,
  base: str,
  branch: str,
  title: str,
  body: str,
  bodyHtml: str,
  patch: optional(Patch),
});
export type NewPullRequest = Infer<typeof NewPullRequest>;

export const Proposal = shape({
  id: str,
  summary: str,
  pullRequestUrl: str,
  comment: optional(Comment),
  threadId: str,
  patch: optional(Patch),
  reply: str,
  resolveThread: bool,
  error: str,
  requestedTool: str,
  earlierComments: list(Comment),
  replyHtml: str,
  followUp: str,
  followUpHtml: str,
  closePullRequest: bool,
  checkFailure: optional(CheckFailure),
  rerun: bool,
  summaryHtml: str,
  investigationId: str,
  otherPatches: list(Patch),
  openPullRequest: optional(NewPullRequest),
});
export type Proposal = Infer<typeof Proposal>;

export const PullRequest = shape({
  repository: optional(Repository),
  number: num,
  title: str,
  url: str,
  approved: bool,
  checksPassing: bool,
  checksFailing: bool,
  proposals: list(Proposal),
  behind: bool,
  checks: optional(Checks),
  headOid: str,
  mergeBy: enumOf<MergeBy>('MERGE_BY_UNSPECIFIED'),
  files: list(FileDiff),
  base: str,
  conflicts: bool,
  rebasing: bool,
  rebased: bool,
  merging: bool,
  noProposalReason: str,
  createdAt: time,
});
export type PullRequest = Infer<typeof PullRequest>;

export const Agent = shape({
  id: str,
  title: str,
  state: enumOf<AgentState>('AGENT_STATE_WORKING'),
  pullRequestUrls: list(str),
  reply: str,
  replyHtml: str,
  proposals: list(Proposal),
  startedAt: time,
});
export type Agent = Infer<typeof Agent>;

export const Cluster = shape({
  // Oldest first.
  pullRequests: list(PullRequest),
  updatedAt: time,
  name: str,
  mergeBy: enumOf<MergeBy>('MERGE_BY_UNSPECIFIED'),
  agents: list(Agent),
});
export type Cluster = Infer<typeof Cluster>;

// A moment of the view.
export const Moment = shape({
  // Each cluster by its key, the URL of its oldest pull request.
  clusters: keyed(Cluster),
  // The clusters' keys, most recently updated first.
  order: list(str),
  login: str,
  search: str,
  // The agents acting on no open pull request.
  agents: list(Agent),
  // The actions in progress, each by the key the frontend names it by (a proposal's ID, a
  // cluster's key, `merge` or `rebase` and a cluster's key, a pull request's URL, or an agent's
  // ID), with its route.
  pending: keyed(str),
});
export type Moment = Infer<typeof Moment>;

// What the UI shows of a moment: the clusters in order, and the rest. A cluster's key, which
// links to it, is the URL of its oldest pull request.
export function shown(moment: Moment | undefined) {
  if (!moment) return { clusters: [], agents: [], login: '', search: '' };
  return {
    clusters: moment.order.flatMap((key) => moment.clusters.get(key) ?? []),
    agents: moment.agents,
    login: moment.login,
    search: moment.search,
  };
}
