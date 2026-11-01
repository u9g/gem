import { type Agent } from './view-types';

// Queued or working, rather than waiting on the user.
export function active(agent: Agent): boolean {
  return agent.state !== 'waiting';
}
