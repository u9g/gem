import { type Agent, AgentState, type PullRequestCluster } from './gen/gem_pb';

// Queued or working, rather than waiting on the user.
export function active(agent: Agent): boolean {
  return agent.state !== AgentState.WAITING;
}

// The active agents acting on any of the cluster's pull requests, which its view lists.
export function agentsOf(cluster: PullRequestCluster, agents: Agent[]): Agent[] {
  const urls = new Set(cluster.pullRequests.map((pr) => pr.url));
  return agents.filter((agent) => active(agent) && agent.pullRequestUrls.some((url) => urls.has(url)));
}

// The agents the agent list shows: every one but those an open cluster's view lists. Those wait on
// the user, or belong to no open cluster, so the list is where they are followed up.
export function unclustered(agents: Agent[], clusters: PullRequestCluster[]): Agent[] {
  const urls = new Set(clusters.flatMap((cluster) => cluster.pullRequests.map((pr) => pr.url)));
  return agents.filter((agent) => !active(agent) || !agent.pullRequestUrls.some((url) => urls.has(url)));
}
