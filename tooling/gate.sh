#!/usr/bin/env bash
# Runs every check a change must pass before it is done, and times each one.
#
#   tooling/gate.sh
#
# It prints one line per check and writes them to target/gate/summary.txt.
# Each check's full output goes to target/gate/<check>.txt. It runs every
# check even when one fails, and exits 1 if any failed.
#
# The order is the fastest safe one we found (docs/operations/TEST_TIMES.md):
#   1. fmt, then clippy, then wasm clippy. cargo lets one build at a time
#      use target/, so running them together would only make them wait.
#   2. Build everything the tests run: the journey app, the Rust tests and
#      the catalog tools. Again one cargo build at a time.
#   3. Run the Rust tests, JavaScript tests, CSV checks, pilots and
#      ml doctor side by side. Nothing builds here, so nothing waits for
#      the cargo lock.
#   4. The journeys, with nothing else running: some of them check how
#      fast the app answers.
#
# The Rust tests run under cargo-nextest, pinned in infra/versions.lock and
# downloaded once into target/tools/nextest. nextest runs each test in its
# own process, all side by side, which takes about 65 s instead of 340 s.
# Without it (no download, or an unknown platform) the gate uses cargo test.
#
# Works in Git Bash on Windows and in bash on macOS and Linux (bash 3.2 up).

set -u
cd "$(dirname "$0")/.." || exit 1

out=target/gate
summary=$out/summary.txt
mkdir -p "$out"
: >"$summary"

exe=''
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) exe='.exe' ;;
esac

say() {
  echo "$*" | tee -a "$summary"
}

# check <name> <command...>: runs the command, with its output in
# $out/<name>.txt, and writes "<name>: ok|FAILED <seconds> s" to
# $out/<name>.result. The caller prints the results in a fixed order, so
# checks that run side by side do not mix their lines.
check() {
  local name=$1
  shift
  local started=$SECONDS
  local result=ok
  "$@" >"$out/$name.txt" 2>&1 || result=FAILED
  echo "$name: $result $((SECONDS - started)) s$(detail "$name")" >"$out/$name.result"
}

# report <name...>: prints the results of checks that have finished.
report() {
  local name
  for name in "$@"; do
    cat "$out/$name.result" | tee -a "$summary"
    if grep -q ': FAILED' "$out/$name.result"; then
      failed=1
      echo "  see $out/$name.txt" | tee -a "$summary"
    fi
  done
}

# detail <name>: a short summary from the check's own output, when it has one.
detail() {
  local log=$out/$1.txt line=''
  case $1 in
    rust-tests) line=$(grep -E '^ *Summary \[|^test result:' "$log" | tail -1) ;;
    js-tests) line=$(grep -E '^ℹ (pass|fail) ' "$log" | tr '\n' ' ') ;;
    pilots) line=$(tail -1 "$log") ;;
    full-pilot) line=$(grep -m1 -E '^(PASSED|FAILED)' "$log" | cut -c1-60) ;;
    journeys) line=$(grep -E '^journeys:' "$log" | sed 's/^journeys: //') ;;
    ml-doctor) line=$(tail -1 "$log") ;;
  esac
  if [ -n "$line" ]; then echo " ($(echo "$line" | sed 's/^ *//; s/ *$//'))"; fi
}

# lock_value <key>: a value from infra/versions.lock, such as nextest.
lock_value() {
  sed -n "s/^$1 = \"\(.*\)\"$/\1/p" infra/versions.lock
}

sha256_of() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# get_nextest: prints the folder holding the pinned cargo-nextest,
# downloading it the first time. Prints nothing if it cannot.
get_nextest() {
  local platform
  case "$(uname -s)-$(uname -m)" in
    MINGW*-x86_64 | MSYS*-x86_64 | CYGWIN*-x86_64) platform=windows_x64 ;;
    Linux-x86_64) platform=linux_x64 ;;
    Darwin-*) platform=macos ;;
    *) return 0 ;;
  esac
  local version url sha256 dir archive
  version=$(lock_value nextest)
  url=$(lock_value "nextest_${platform}_url")
  sha256=$(lock_value "nextest_${platform}_sha256")
  dir=target/tools/nextest/$version
  if [ -x "$dir/cargo-nextest$exe" ]; then
    echo "$dir"
    return 0
  fi
  # A new folder each time, so a download that stopped halfway is never used.
  local staging=$dir.partial-$$
  rm -rf "$staging"
  mkdir -p "$staging"
  archive=$staging/${url##*/}
  if ! curl -fsSL --retry 2 -o "$archive" "$url"; then
    echo "could not download $url" >&2
  elif [ "$(sha256_of "$archive")" != "$sha256" ]; then
    echo "$url does not have the SHA-256 that infra/versions.lock pins" >&2
  else
    case $archive in
      # Windows' own tar.exe reads ZIP files; Git Bash's GNU tar does not.
      *.zip) "${SYSTEMROOT:-C:/Windows}/System32/tar.exe" -xf "$archive" -C "$staging" ;;
      *) tar -xzf "$archive" -C "$staging" ;;
    esac
    rm -f "$archive"
    rm -rf "$dir"
    mv "$staging" "$dir" && echo "$dir"
  fi
  rm -rf "$staging"
}

say "gate on $(git rev-parse --abbrev-ref HEAD) at $(git rev-parse --short HEAD), started $(date '+%Y-%m-%d %H:%M:%S')"
gate_started=$SECONDS
failed=0

nextest_dir=$(get_nextest)
if [ -n "$nextest_dir" ]; then
  PATH="$PWD/$nextest_dir:$PATH"
  build_rust_tests() { cargo nextest run --workspace --locked --no-run; }
  run_rust_tests() { cargo nextest run --workspace --locked --no-fail-fast; }
  say "Rust tests run with cargo-nextest $(lock_value nextest)"
else
  build_rust_tests() { cargo test --workspace --locked --no-run; }
  run_rust_tests() { cargo test --workspace --locked --no-fail-fast; }
  say "Rust tests run with cargo test: cargo-nextest is not available here"
fi

# 1. Lint.
check fmt cargo fmt --all --check
check clippy cargo clippy --workspace --all-targets --locked -- -D warnings
check clippy-wasm cargo clippy -p kicad-parts-desktop-ui --target wasm32-unknown-unknown --locked -- -D warnings
report fmt clippy clippy-wasm

# 2. Build. catalog-import is part of the journey app build.
check build-journey-app node apps/desktop/journeys/run-journeys.mjs --build-only
check build-rust-tests build_rust_tests
check build-csv-fixture cargo build --locked -p csv-catalog-fixture
report build-journey-app build-rust-tests build-csv-fixture

# 3. Tests that need no app, side by side. The pilots run one after the
# other: each starts its own team server, and together they are short.
pilots() {
  local file name failures=0 count=0
  for file in database/g-*.csv; do
    name=$(basename "$file" .csv)
    count=$((count + 1))
    if ! "target/debug/catalog-import$exe" pilot . "$name" >"$out/pilot-$name.txt" 2>&1; then
      echo "pilot $name FAILED: see $out/pilot-$name.txt"
      failures=$((failures + 1))
    fi
  done
  echo "$failures of $count pilots failed"
  [ "$failures" -eq 0 ]
}
check rust-tests run_rust_tests &
check js-tests npm run -s js-tests --prefix apps/desktop/journeys &
check csv-fixture "target/debug/csv-catalog-fixture$exe" check . &
check pilots pilots &
check full-pilot "target/debug/catalog-import$exe" full-pilot &
check ml-doctor ml doctor &
wait
report rust-tests js-tests csv-fixture pilots full-pilot ml-doctor

# 4. Journeys, alone.
check journeys node apps/desktop/journeys/run-journeys.mjs --skip-build
report journeys

total=$((SECONDS - gate_started))
if [ "$failed" -eq 0 ]; then
  say "gate: ok in $((total / 60)) min $((total % 60)) s"
else
  say "gate: FAILED in $((total / 60)) min $((total % 60)) s"
fi
exit "$failed"
