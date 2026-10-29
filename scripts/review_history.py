# /// script
# requires-python = ">=3.11"
# ///
"""Turns how the user treated review comments on their merged pull requests into decisions for
tune_decision_prompt.py.

Each review thread someone else started becomes a proposal to change the code as its comment
asks. A labeller reads the user's replies and what changed near the comment afterwards: approved
when the user acted on it, dismissed when they declined or ignored it. Comments that ask for no
change, such as questions or praise, are left out.

    uv run scripts/review_history.py --since 2026-07-01 --out review-history.sqlite
"""

import argparse, json, re, sqlite3, subprocess, sys
from concurrent.futures import ThreadPoolExecutor
from datetime import date, timedelta
from pathlib import Path

from tune_decision_prompt import claude

MIGRATION = Path(__file__).parent.parent / "server/crates/db/migrations/20261028235609_create_proposal_decisions.sql"

QUERY = """
query($q: String!, $after: String) {
  search(query: $q, type: ISSUE, first: 25, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes { ... on PullRequest {
      url headRefOid mergedAt
      reviewThreads(first: 100) { nodes {
        path originalLine
        comments(first: 20) { nodes {
          id body createdAt diffHunk author { login } originalCommit { oid }
        } }
      } }
    } }
  }
}"""


def gh(*args):
    return json.loads(subprocess.run(["gh", *args], capture_output=True, text=True, check=True).stdout)


def pull_requests(login, since, until):
    # Search returns at most 1000 results, so ask a week at a time.
    day = since
    while day < until:
        end = min(day + timedelta(days=7), until)
        q = f"author:{login} is:pr is:merged merged:{day}..{end - timedelta(days=1)}"
        after = None
        while True:
            args = ["api", "graphql", "-f", f"query={QUERY}", "-f", f"q={q}"]
            if after:
                args += ["-f", f"after={after}"]
            search = gh(*args)["data"]["search"]
            yield from (n for n in search["nodes"] if n)
            if not search["pageInfo"]["hasNextPage"]:
                break
            after = search["pageInfo"]["endCursor"]
        day = end


# Bots wrap their findings in HTML comments and badges.
def clean(body):
    body = re.sub(r"<!--.*?-->", "", body, flags=re.S)
    body = re.sub(r"<a href=.*?</a>", "", body, flags=re.S)
    return re.sub(r"\n---\n\*Was this helpful.*", "", body, flags=re.S).strip()


LABEL_SCHEMA = {
    "type": "object",
    "properties": {
        "reason": {"type": "string"},
        "label": {"enum": ["acted", "declined", "not_actionable"]},
    },
    "required": ["reason", "label"],
}

LABELLER = """A reviewer commented on the user's pull request. From the user's replies and the changes \
made near the commented line afterwards, decide what the user did with the comment:
- acted: changed the code as the comment asks, or the way it points at, even partly.
- declined: the comment asks for a change, and the user pushed back, resolved it without one, or \
ignored it.
- not_actionable: the comment asks for no change, such as a question, praise, an aside or a note, \
whether or not the user replied."""


# The hunks of `patch` whose old side is within `margin` lines of `line`.
def near(patch, line, margin=20):
    hunks = re.split(r"(?m)^(?=@@ )", patch or "")
    keep = []
    for hunk in hunks:
        m = re.match(r"@@ -(\d+)(?:,(\d+))?", hunk)
        if m and line:
            start, length = int(m[1]), int(m[2] or 1)
            if start - margin <= line <= start + length + margin:
                keep.append(hunk)
    return "".join(keep)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--login", default=None)
    parser.add_argument("--since", type=date.fromisoformat, required=True)
    parser.add_argument("--until", type=date.fromisoformat, default=date.today() + timedelta(days=1))
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--labeller", default="claude-sonnet-5-5")
    parser.add_argument("--workers", type=int, default=8)
    args = parser.parse_args()
    login = args.login or gh("api", "user")["login"]

    db = sqlite3.connect(args.out, check_same_thread=False)
    db.executescript(MIGRATION.read_text().replace("CREATE TABLE", "CREATE TABLE IF NOT EXISTS"))
    done = {id for (id,) in db.execute("SELECT id FROM proposal_decisions")}
    files = {}

    def changes(repo, base, head, path):
        if (repo, base, head) not in files:
            try:
                compare = gh("api", f"repos/{repo}/compare/{base}...{head}")
                files[repo, base, head] = {f["filename"]: f.get("patch", "") for f in compare.get("files", [])}
            except subprocess.CalledProcessError:
                files[repo, base, head] = {}
        return files[repo, base, head].get(path, "")

    def threads():
        for pr in pull_requests(login, args.since, args.until):
            repo = "/".join(pr["url"].split("/")[3:5])
            for thread in pr["reviewThreads"]["nodes"]:
                comments = thread["comments"]["nodes"]
                if not comments or (comments[0]["author"] or {}).get("login") in (login, None):
                    continue
                if comments[0]["id"] not in done:
                    yield pr, repo, thread, comments

    def label(job):
        pr, repo, thread, comments = job
        first = comments[0]
        changed = near(changes(repo, first["originalCommit"]["oid"], pr["headRefOid"], thread["path"]),
                       thread["originalLine"])
        conversation = "\n\n".join(f"{(c['author'] or {}).get('login', '')}: {clean(c['body'])}" for c in comments)
        verdict = claude(args.labeller, LABELLER,
                         f"<thread path={thread['path']!r} line={thread['originalLine']}>\n{first['diffHunk']}\n\n"
                         f"{conversation}\n</thread>\n\n<changed_after>\n{changed or '(nothing near the line)'}\n</changed_after>",
                         LABEL_SCHEMA)
        return job, verdict if isinstance(verdict, dict) else json.loads(verdict)

    counts = {"approved": 0, "dismissed": 0, "not_actionable": 0}
    with ThreadPoolExecutor(args.workers) as pool:
        for (pr, repo, thread, comments), verdict in pool.map(label, threads()):
            first = comments[0]
            if verdict["label"] == "not_actionable":
                counts["not_actionable"] += 1
                continue
            decision = "approved" if verdict["label"] == "acted" else "dismissed"
            # The reviewer's side of the thread, up to the user's first answer.
            asked = []
            for c in comments:
                if (c["author"] or {}).get("login") == login:
                    break
                asked.append(c)
            author = first["author"]["login"]
            proposal = {
                "summary": f"Change the code as {author} asks in this review comment",
                "pull_request_url": pr["url"],
                "comment": {"author": author, "body": clean(asked[-1]["body"]), "path": thread["path"],
                            "diff_hunk": first["diffHunk"]},
                "earlier_comments": [{"author": (c["author"] or {}).get("login", ""), "body": clean(c["body"])}
                                     for c in asked[:-1]],
            }
            ts = int(__import__("datetime").datetime.fromisoformat(first["createdAt"].replace("Z", "+00:00")).timestamp())
            db.execute("INSERT OR REPLACE INTO proposal_decisions (id, proposal, decision, decided_at) VALUES (?,?,?,?)",
                       (first["id"], json.dumps(proposal), decision, ts))
            db.commit()
            counts[decision] += 1
            print(f"\r{counts}", end="", file=sys.stderr)
    print(file=sys.stderr)


if __name__ == "__main__":
    main()
