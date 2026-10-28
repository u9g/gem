CREATE TABLE merges (
    url TEXT PRIMARY KEY,
    -- Shared by the cluster's pull requests. NULL while the thread is starting.
    thread_id TEXT,
    question INTEGER NOT NULL DEFAULT 0
);
