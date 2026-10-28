-- The thread's title, and when the agent started in seconds since the epoch, so the merge shows
-- like any other agent.
ALTER TABLE merges ADD COLUMN title TEXT NOT NULL DEFAULT '';
ALTER TABLE merges ADD COLUMN started_at INTEGER NOT NULL DEFAULT 0;
