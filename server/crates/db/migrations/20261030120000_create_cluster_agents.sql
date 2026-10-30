-- The thread of the agent answering the comments on each cluster, by the URL of the cluster's
-- oldest pull request when it started.
CREATE TABLE cluster_agents (pr TEXT PRIMARY KEY, thread_id TEXT NOT NULL);

-- Agents answering one comment each no longer propose: their tokens go, and the comments they had
-- proposed nothing for yet are answered again by their clusters' agents. Failed checks' IDs hold
-- a comma; comments' do not.
DELETE FROM agent_tokens WHERE json_extract(subject, '$.comment') IS NOT NULL;
DELETE FROM started_conversations
WHERE id NOT LIKE '%,%'
  AND id NOT IN (
    SELECT json_extract(proposal, '$.comment.id') FROM proposals
    WHERE json_extract(proposal, '$.comment.id') IS NOT NULL
  );
