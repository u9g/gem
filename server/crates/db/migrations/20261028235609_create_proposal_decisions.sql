-- What the user decided on each proposal they approved or dismissed, kept after the proposal is
-- gone. Proposals cleared without the user deciding on them, such as stale ones, have no row.
CREATE TABLE proposal_decisions (
    id TEXT PRIMARY KEY,
    -- The Proposal message as the user saw it when deciding, as JSON.
    proposal TEXT NOT NULL,
    -- 'approved' or 'dismissed'.
    decision TEXT NOT NULL,
    -- The user's rewrites approved in place of the proposal's own; empty when approved as proposed.
    reply TEXT NOT NULL DEFAULT '',
    follow_up TEXT NOT NULL DEFAULT '',
    diff TEXT NOT NULL DEFAULT '',
    -- Seconds since the epoch.
    decided_at INTEGER NOT NULL
);
