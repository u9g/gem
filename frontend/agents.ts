import type { Node } from './db';
import { type Agent, agentState } from './shapes';

// Queued or working, rather than waiting on the user.
export function active(agent: Node<Agent>): boolean {
  return agentState(agent) !== 'AGENT_STATE_WAITING';
}
