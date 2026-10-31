// What the server's view database holds: the structs of server/src/view/types.rs, as the reader in
// db.ts reads them. Every field is stored, so each is here as it is there; only an `Option` can be
// missing, which is an optional property. Times are milliseconds since the epoch.

export type MergeBy = 'unspecified' | 'approval' | 'admin';
export type AgentState = 'working' | 'queued' | 'waiting';

// A moment of the view.
export interface Moment {
  // Each cluster by its key, the URL of its oldest pull request.
  clusters: Readonly<Record<string, Cluster>>;
  // The clusters' keys, most recently updated first.
  order: readonly string[];
  login: string;
  search: string;
  // The agents acting on no open pull request.
  agents: readonly Agent[];
  // The actions in progress, each by the key the frontend names it by (a proposal's ID, a
  // cluster's key, `merge` or `rebase` and a cluster's key, a pull request's URL, or an agent's
  // ID), with its route.
  pending: Readonly<Record<string, string>>;
}

export interface Cluster {
  // Oldest first.
  readonly pullRequests: readonly PullRequest[];
  readonly updatedAt?: number;
  readonly name: string;
  readonly mergeBy: MergeBy;
  readonly agents: readonly Agent[];
}

export interface PullRequest {
  readonly repository?: Repository;
  readonly number: number;
  readonly title: string;
  readonly url: string;
  readonly approved: boolean;
  readonly checksPassing: boolean;
  readonly checksFailing: boolean;
  readonly proposals: readonly Proposal[];
  readonly behind: boolean;
  readonly checks?: Checks;
  readonly headOid: string;
  readonly mergeBy: MergeBy;
  readonly files: readonly FileDiff[];
  readonly base: string;
  readonly conflicts: boolean;
  readonly rebasing: boolean;
  readonly rebased: boolean;
  readonly merging: boolean;
  readonly noProposalReason: string;
  readonly createdAt?: number;
}

export interface Checks {
  readonly total: number;
  readonly done: number;
  readonly failed: number;
}

export interface Repository {
  readonly owner: string;
  readonly name: string;
}

export interface Agent {
  readonly id: string;
  readonly title: string;
  readonly state: AgentState;
  readonly pullRequestUrls: readonly string[];
  readonly reply: string;
  readonly replyHtml: string;
  readonly proposals: readonly Proposal[];
  readonly startedAt?: number;
}

export interface Proposal {
  readonly id: string;
  readonly summary: string;
  readonly pullRequestUrl: string;
  readonly comment?: Comment;
  readonly threadId: string;
  readonly patch?: Patch;
  readonly reply: string;
  readonly resolveThread: boolean;
  readonly error: string;
  readonly requestedTool: string;
  readonly earlierComments: readonly Comment[];
  readonly replyHtml: string;
  readonly followUp: string;
  readonly followUpHtml: string;
  readonly closePullRequest: boolean;
  readonly checkFailure?: CheckFailure;
  readonly rerun: boolean;
  readonly summaryHtml: string;
  readonly investigationId: string;
  readonly otherPatches: readonly Patch[];
  readonly openPullRequest?: NewPullRequest;
}

export interface NewPullRequest {
  readonly repository: string;
  readonly base: string;
  readonly branch: string;
  readonly title: string;
  readonly body: string;
  readonly bodyHtml: string;
  readonly patch?: Patch;
}

export interface CheckFailure {
  readonly id: string;
  readonly headOid: string;
  readonly checks: readonly FailedCheck[];
}

export interface FailedCheck {
  readonly id: string;
  readonly name: string;
  readonly conclusion: string;
  readonly url: string;
  readonly workflowRunId: number;
  readonly excerpt: string;
}

export interface Comment {
  readonly id: string;
  readonly author: string;
  readonly body: string;
  readonly url: string;
  readonly bodyHtml: string;
  readonly path: string;
  readonly line: number;
  readonly diffHunk: string;
}

export interface FileDiff {
  readonly path: string;
  readonly patch: string;
}

export interface Patch {
  readonly diff: string;
  readonly commitMessage: string;
  readonly headOid: string;
  readonly pullRequestUrl: string;
}
