-- The thread of the agent answering the comments on each cluster, by the URL of the cluster's
-- oldest pull request when it started.
CREATE TABLE cluster_agents (pr TEXT PRIMARY KEY, thread_id TEXT NOT NULL);
