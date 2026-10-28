CREATE TABLE rebases (
    url TEXT PRIMARY KEY,
    -- The head commit when the rebase started.
    head TEXT NOT NULL,
    -- NULL while the thread is starting.
    thread_id TEXT,
    question INTEGER NOT NULL DEFAULT 0,
    pushed INTEGER NOT NULL DEFAULT 0
);
