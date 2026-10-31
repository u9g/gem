import { type Agent } from './shapes';

// Queued or working, rather than waiting on the user.
export function active(agent: Agent): boolean {
  return agent.state !== 'AGENT_STATE_WAITING';
}
