import { type Agent, AgentState } from './gen/gem_pb';

// Queued or working, rather than waiting on the user.
export function active(agent: Agent): boolean {
  return agent.state !== AgentState.WAITING;
}
