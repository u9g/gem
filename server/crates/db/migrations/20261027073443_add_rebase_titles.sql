-- The thread's title, and when the agent started in seconds since the epoch, so the rebase shows
-- like any other agent.
ALTER TABLE rebases ADD COLUMN title TEXT NOT NULL DEFAULT '';
ALTER TABLE rebases ADD COLUMN started_at INTEGER NOT NULL DEFAULT 0;
