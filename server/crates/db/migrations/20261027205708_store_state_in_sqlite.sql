-- The state the server kept in files in its working directory.

-- Cluster names, by `names::key` of the cluster's pull requests and titles.
CREATE TABLE cluster_names (
    key TEXT PRIMARY KEY,
    name TEXT NOT NULL
);

-- The conversations an agent was started for.
CREATE TABLE started_conversations (id TEXT PRIMARY KEY);

-- Why each agent answering failed checks proposed nothing, by the ID of the checks it answered.
CREATE TABLE no_proposal_reasons (
    id TEXT PRIMARY KEY,
    reason TEXT NOT NULL
);

CREATE TABLE proposals (
    id TEXT PRIMARY KEY,
    -- The Proposal message, as JSON.
    proposal TEXT NOT NULL
);

CREATE TABLE investigations (
    id TEXT PRIMARY KEY,
    -- The Investigation, as JSON.
    investigation TEXT NOT NULL,
    -- A closed investigation stays, so later agents can search what it found.
    closed INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE agent_tokens (
    token TEXT PRIMARY KEY,
    -- A tokens::Subject, as JSON.
    subject TEXT NOT NULL
);

-- The clusters as gem last showed them, with their names and proposals: one row.
CREATE TABLE snapshot (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    -- The ListOpenPullRequestsResponse, as JSON.
    response TEXT NOT NULL
);
