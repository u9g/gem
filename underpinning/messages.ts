import { create, type DescMessage, type JsonValue, toJson } from '@bufbuild/protobuf';

import * as gem from './gen/gem_pb';

// A request the gem server's router answers (server/src/main.rs), with the message its body is.
export type Message = { method: 'GET' | 'POST'; path: string; label: string; request?: DescMessage };

export const messages: Message[] = [
  { method: 'GET', path: '/healthz', label: 'Health' },
  { method: 'GET', path: '/repos', label: 'Repositories' },
  { method: 'POST', path: '/prs', label: 'List pull requests', request: gem.ListOpenPullRequestsRequestSchema },
  { method: 'POST', path: '/prs/search', label: 'Search pull requests', request: gem.SearchPullRequestsRequestSchema },
  { method: 'POST', path: '/prs/rebase', label: 'Rebase pull request', request: gem.RebasePullRequestRequestSchema },
  { method: 'POST', path: '/clusters/message', label: 'Message cluster', request: gem.MessageClusterRequestSchema },
  { method: 'POST', path: '/clusters/restart', label: 'Restart cluster', request: gem.RestartClusterRequestSchema },
  { method: 'POST', path: '/clusters/rebase', label: 'Rebase cluster', request: gem.RebaseClusterRequestSchema },
  { method: 'POST', path: '/clusters/merge', label: 'Merge cluster', request: gem.MergeClusterRequestSchema },
  { method: 'POST', path: '/proposals/approve', label: 'Approve proposal', request: gem.ApproveProposalRequestSchema },
  { method: 'POST', path: '/proposals/dismiss', label: 'Dismiss proposal', request: gem.DismissProposalRequestSchema },
  { method: 'POST', path: '/investigations/start', label: 'Investigate', request: gem.InvestigateRequestSchema },
  { method: 'POST', path: '/agents/message', label: 'Message agent', request: gem.MessageAgentRequestSchema },
  { method: 'POST', path: '/agents/close', label: 'Close agent', request: gem.CloseAgentRequestSchema },
  { method: 'POST', path: '/markdown/render', label: 'Render markdown', request: gem.RenderMarkdownRequestSchema },
];

// Whether the request has fields to fill in before it is sent.
export function hasFields(message: Message): boolean {
  return (message.request?.fields.length ?? 0) > 0;
}

// The request's body with every field present and empty, for filling in.
export function template(message: Message): JsonValue {
  return message.request ? toJson(message.request, create(message.request), { alwaysEmitImplicit: true }) : null;
}
