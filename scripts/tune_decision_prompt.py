# /// script
# requires-python = ">=3.11"
# ///
"""Tunes a prompt that predicts whether the user approves or dismisses a proposal.

A champion prompt judges every recorded decision. Each round, an optimizer reads the champion's
misses and writes a challenger; both judge the same decisions, and the challenger replaces the
champion only when its balanced accuracy is higher. Stops once the champion predicts every
decision, or after --rounds rounds.

    uv run scripts/tune_decision_prompt.py --db server/gem.sqlite [--db more.sqlite] --out prompt.txt
"""

import argparse, hashlib, json, random, sqlite3, subprocess, sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

SEED = """You review proposals that an AI agent makes on the user's GitHub pull requests: \
replies to reviewers, patches, follow-up prompts for other agents, check reruns. Each one runs \
as the user if they approve it. Predict whether the user approves or dismisses it. The user \
approves proposals that answer the root cause of what they respond to, and dismisses ones that \
work around a symptom, guess, or do more or less than was asked."""

SCHEMA = {
    "type": "object",
    "properties": {
        "reason": {"type": "string"},
        "decision": {"enum": ["approved", "dismissed"]},
    },
    "required": ["reason", "decision"],
}

# Rendered in proposals for the UI only.
HIDDEN = {"id", "error", "reply_html", "follow_up_html", "summary_html", "body_html", "head_oid"}


def strip(value):
    if isinstance(value, dict):
        return {k: strip(v) for k, v in value.items() if k not in HIDDEN and v not in ("", None, [], False)}
    if isinstance(value, list):
        return [strip(v) for v in value]
    return value


def claude(model, system, prompt, schema=None):
    command = ["claude", "-p", "--model", model, "--system-prompt", system, "--tools", "",
               "--strict-mcp-config", "--setting-sources", "", "--no-session-persistence",
               "--output-format", "json"]
    if schema:
        command += ["--json-schema", json.dumps(schema)]
    out = subprocess.run(command, input=prompt, capture_output=True, text=True, check=True).stdout
    result = json.loads(out)
    return result.get("structured_output") or result["result"]


def load(paths):
    decisions = {}
    for path in paths:
        db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
        for id, proposal, decision in db.execute("SELECT id, proposal, decision FROM proposal_decisions"):
            text = json.dumps(strip(json.loads(proposal)), indent=1)
            decisions[id] = {"id": id, "proposal": text[:40000], "decision": decision}
    return list(decisions.values())


class Judge:
    def __init__(self, model, cache_path):
        self.model, self.cache_path = model, cache_path
        self.cache = json.loads(cache_path.read_text()) if cache_path.exists() else {}

    def __call__(self, prompt, item):
        key = hashlib.sha256(f"{self.model}\0{prompt}\0{item['proposal']}".encode()).hexdigest()
        if key not in self.cache:
            verdict = claude(self.model, prompt, item["proposal"], SCHEMA)
            if isinstance(verdict, str):
                verdict = json.loads(verdict)
            self.cache[key] = verdict
        return self.cache[key]

    def score(self, prompt, items, workers):
        with ThreadPoolExecutor(workers) as pool:
            verdicts = list(pool.map(lambda item: self(prompt, item), items))
        self.cache_path.write_text(json.dumps(self.cache))
        results = [{**item, **{"predicted": v["decision"], "reason": v["reason"]}} for item, v in zip(items, verdicts)]
        # Balanced, so a prompt that approves everything scores 0.5 however rare dismissals are.
        recalls = [
            sum(r["predicted"] == label for r in results if r["decision"] == label)
            / max(1, sum(r["decision"] == label for r in results))
            for label in ("approved", "dismissed")
        ]
        return sum(recalls) / 2, results


def challenger(model, champion, results):
    misses = [r for r in results if r["predicted"] != r["decision"]]
    hits = random.sample([r for r in results if r["predicted"] == r["decision"]],
                         k=min(4, len(results) - len(misses)))
    show = lambda r: (f"<proposal>\n{r['proposal'][:6000]}\n</proposal>\nThe user {r['decision']} it. "
                      f"The prompt predicted {r['predicted']}: {r['reason']}")
    prompt = (
        f"This system prompt predicts whether the user approves or dismisses an agent's proposal "
        f"on their pull requests:\n\n<prompt>\n{champion}\n</prompt>\n\n"
        f"It got these wrong:\n\n" + "\n\n".join(map(show, misses)) +
        "\n\nAnd these right:\n\n" + "\n\n".join(map(show, hits)) +
        "\n\nWork out what the user's taste is that the prompt is missing, and rewrite the prompt "
        "to capture it as general principles a judge can apply to proposals it has never seen. "
        "Do not name these proposals, repositories or tests. Reply with only the new prompt."
    )
    return claude(model, "You write prompts for an LLM judge.", prompt).strip()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--db", action="append", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--rounds", type=int, default=10)
    parser.add_argument("--judge", default="claude-sonnet-5-5")
    parser.add_argument("--optimizer", default="claude-opus-5-5")
    parser.add_argument("--workers", type=int, default=8)
    args = parser.parse_args()

    items = load(args.db)
    print(f"{len(items)} decisions: {sum(i['decision'] == 'dismissed' for i in items)} dismissed", file=sys.stderr)
    judge = Judge(args.judge, args.out.with_suffix(".cache.json"))
    champion = args.out.read_text() if args.out.exists() else SEED
    best, results = judge.score(champion, items, args.workers)
    print(f"champion: {best:.3f}", file=sys.stderr)
    for round in range(1, args.rounds + 1):
        if best == 1:
            break
        prompt = challenger(args.optimizer, champion, results)
        score, challenger_results = judge.score(prompt, items, args.workers)
        won = score > best
        print(f"round {round}: challenger {score:.3f} vs champion {best:.3f}{' -> new champion' if won else ''}",
              file=sys.stderr)
        if won:
            champion, best, results = prompt, score, challenger_results
            args.out.write_text(champion)
    args.out.write_text(champion)
    for r in results:
        if r["predicted"] != r["decision"]:
            print(f"miss {r['id']}: user {r['decision']}, predicted {r['predicted']}: {r['reason']}", file=sys.stderr)


if __name__ == "__main__":
    main()
