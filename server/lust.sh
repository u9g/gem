#!/usr/bin/env bash
# Checks and builds the server with lust (https://github.com/t3dotgg/lust-lang). With CARGO_SERVER
# naming a Cargo-built server (and GEM_NATIVE the gem-native built with it), also checks that the
# lust-built server answers MCP requests on a state directory exactly as the Cargo-built one does.
# Reports each step and fails if any failed. (`lust test` builds single files only, so the tests
# run under Cargo.)
cd "$(dirname "$0")"
failed=()
step() {
  echo "::group::$*"
  if "$@"; then echo "::endgroup::"; else echo "::endgroup::"; echo "FAILED: $*"; failed+=("$*"); fi
}

# The same MCP requests to both servers, on copies of one state directory holding a snapshot.
parity() {
  local state
  state=$(mktemp -d)
  # The first run makes the database; the snapshot then goes in as the running server saves it.
  "$CARGO_SERVER" mcp --state "$state" < /dev/null || return 1
  sqlite3 "$state/gem.sqlite" "INSERT OR REPLACE INTO snapshot (id, response) VALUES (0, CAST(readfile('tests/lust/snapshot.json') AS TEXT))" || return 1
  cp -r "$state" "$state-lust"
  "$CARGO_SERVER" mcp --state "$state" < tests/lust/requests.jsonl > "$state.cargo.jsonl" || return 1
  target/gem-server-lust mcp --state "$state-lust" < tests/lust/requests.jsonl > "$state.lust.jsonl" || return 1
  cat "$state.lust.jsonl"
  diff "$state.cargo.jsonl" "$state.lust.jsonl" && [ "$(wc -l < "$state.lust.jsonl")" -eq 7 ]
}

step lust check src/main.rs --bin server --max-errors 0
step lust build src/main.rs --bin server -o target/gem-server-lust
if [ -n "$CARGO_SERVER" ]; then
  step parity
fi
if [ ${#failed[@]} -ne 0 ]; then
  printf 'failed: %s\n' "${failed[@]}"
  exit 1
fi
