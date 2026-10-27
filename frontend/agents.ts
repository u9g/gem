import { type Agent, AgentState, type PullRequestCluster } from './gen/gem_pb';

// Queued or working, rather than waiting on the user.
export function active(agent: Agent): boolean {
  return agent.state !== AgentState.WAITING;
}

// The active agents acting on any of the cluster's pull requests, which its view lists.
export function agentsOf(cluster: PullRequestCluster): Agent[] {
  return cluster.agents.filter(active);
}

// The agents the agent list shows: those acting on no open pull request, and those of a cluster
// waiting on the user, most recently started first.
export function unclustered(agents: Agent[], clusters: PullRequestCluster[]): Agent[] {
  const waiting = clusters.flatMap((cluster) => cluster.agents.filter((agent) => !active(agent)));
  return [...agents, ...waiting].sort((a, b) => Number((b.startedAt?.seconds ?? 0n) - (a.startedAt?.seconds ?? 0n)));
}
