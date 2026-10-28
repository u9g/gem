#!/usr/bin/env bash
# Checks every target lust compiles (https://github.com/t3dotgg/lust-lang), runs the tests of the
# libraries, and builds the server. Reports each and fails if any failed.
cd "$(dirname "$0")"
failed=()
step() {
  echo "::group::$*"
  if "$@"; then echo "::endgroup::"; else echo "::endgroup::"; echo "FAILED: $*"; failed+=("$*"); fi
}
if [ -n "$LUST_PROBE" ]; then
  step lust check crates/probe/src/main.rs --manifest-path crates/probe/Cargo.toml --bin gem-probe
  step lust run crates/probe/src/main.rs --manifest-path crates/probe/Cargo.toml --bin gem-probe
fi
if [ -z "$LUST_SKIP_SERVER" ]; then
  step lust check src/main.rs --bin server
  step lust build src/main.rs --bin server -o target/gem-server-lust
fi
if [ ${#failed[@]} -ne 0 ]; then
  printf 'failed: %s\n' "${failed[@]}"
  exit 1
fi
