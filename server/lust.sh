#!/usr/bin/env bash
# Checks every target lust compiles (https://github.com/t3dotgg/lust-lang), runs the tests of the
# libraries, and builds the server. Reports each and fails if any failed.
cd "$(dirname "$0")"
failed=()
step() {
  echo "::group::$*"
  if "$@"; then echo "::endgroup::"; else echo "::endgroup::"; echo "FAILED: $*"; failed+=("$*"); fi
}
for crate in ${LUST_CRATES:-json cluster}; do
  step lust check "crates/$crate/src/lib.rs" --manifest-path "crates/$crate/Cargo.toml" --lib
  step lust test "crates/$crate/src/lib.rs" --manifest-path "crates/$crate/Cargo.toml" --lib
done
if [ -z "$LUST_SKIP_SERVER" ]; then
  step lust check src/main.rs --bin server
  step lust build src/main.rs --bin server -o target/gem-server-lust
fi
if [ ${#failed[@]} -ne 0 ]; then
  printf 'failed: %s\n' "${failed[@]}"
  exit 1
fi
