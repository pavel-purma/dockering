#!/usr/bin/env bash
# Self-test of the Linux update runner (REL-085; plan linux-appimage-updates §6.5 "selftest"): the harness must FAIL when it should.
# Runs s1.sh, the scenario core of run.sh, against a test double of the app (stub-app.sh, stub_app.py) that carries one known
# defect per control, with stub-atspi.py answering for the accessibility tree. Each control names the checks that must FAIL (and
# the ones that must be SKIPped as a consequence); any other FAIL, a missing FAIL, another SKIP or a zero exit status is a failed
# control. A runner that cannot fail would pass a broken updater. No Docker, no display, no AppImage, no network (127.0.0.1 only):
# the container, the packages and the real AT-SPI calls are the real run's part; this proves the runner's own logic.
# The controls run in parallel, each in its own HOME, port and output directory, as the current user.
#   selftest.sh [--only NAME[,NAME...]] [--out DIR] [--jobs N] [--list]
# Prints one `name | PASS|FAIL | detail` line per control and `== selftest: N passed, M failed`; exits 1 when a control did not behave.
# The evidence (results.txt and the console of every control) is kept in --out (default: a temporary directory, removed when all
# controls behaved).
set -u
usage() { echo "usage: selftest.sh [--only NAME[,NAME...]] [--out DIR] [--jobs N] [--list]" >&2; exit 2; }
bad() { echo "selftest.sh: $*" >&2; usage; }
only="" out="" jobs=4 list=""
while [ $# -gt 0 ]; do
  case $1 in
  --only | --out | --jobs)
    [ $# -ge 2 ] || bad "$1 needs a value"
    case $1 in --only) only=$2 ;; --out) out=$2 ;; --jobs) jobs=$2 ;; esac
    shift 2
    ;;
  --only=*) only=${1#--only=}; shift ;;
  --out=*) out=${1#--out=}; shift ;;
  --jobs=*) jobs=${1#--jobs=}; shift ;;
  --list) list=1; shift ;;
  -h | --help) usage ;;
  *) bad "unexpected argument '$1'" ;;
  esac
done
[[ $jobs =~ ^[1-9][0-9]?$ ]] || bad "--jobs must be 1-99, not '$jobs'"
only_re='^[a-z][a-z0-9_]*(,[a-z][a-z0-9_]*)*$'
[ -z "$only" ] || [[ $only =~ $only_re ]] || bad "--only takes control names (a-z, 0-9, _) separated by commas"
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
for tool in python3 sed awk sha256sum; do command -v "$tool" >/dev/null 2>&1 || { echo "selftest.sh: $tool is required" >&2; exit 1; }; done

OLD_VERSION=0.0.1
NEW_VERSION=0.3.0
# the controls: name | checks that must FAIL | checks that must be SKIPped | E2E_STUB_FLAGS (the defect injected)
CONTROLS=(
  "baseline|||"
  "no_swap|swap||noswap"
  "no_button|button|activate old_exit relaunch swap notice handshake primary_instance no_button_after state_kept|nobutton"
  "exit_early|relaunch|notice handshake primary_instance no_button_after state_kept|exit_early"
  "wrong_user_agent|user_agent||wrong_ua"
  "early_button|before_button||early_button"
  "early_request|before_requests||early_request"
  "old_lingers|old_exit||old_lingers"
  "no_handshake|handshake||no_handshake"
  "not_primary|primary_instance||not_primary"
  "stale_state|state_kept||stale_state"
  "second_check|no_second_check||second_check"
  "no_notice|notice||no_notice"
  "button_again|no_button_after||button_again"
)
names=()
for entry in "${CONTROLS[@]}"; do names+=("${entry%%|*}"); done
if [ -n "$list" ]; then printf '%s\n' "${names[@]}"; exit 0; fi
selected() { [ -z "$only" ] || [[ ",$only," == *",$1,"* ]]; }
for name in ${only//,/ }; do
  [[ " ${names[*]} " == *" $name "* ]] || bad "unknown control '$name' (see --list)"
done

keep=1
if [ -z "$out" ]; then
  out=$(mktemp -d "${TMPDIR:-/tmp}/dockering-update-selftest.XXXXXX") || exit 1
  keep=""
fi
mkdir -p "$out" || exit 1
out=$(cd "$out" && pwd)

# the two stub AppImages and the fixture they serve and download, made like the real fixture's layout
make_stub() { sed -e "s/^ROLE=.*/ROLE=$1/" -e "s/^VERSION=.*/VERSION=$2/" "$here/stub-app.sh" >"$3" && chmod +x "$3"; }
make_stub old "$OLD_VERSION" "$out/old.AppImage" || exit 1
make_stub new "$NEW_VERSION" "$out/new.AppImage" || exit 1
fx="$out/fixture"
repo="$fx/root/pavel-purma/dockering/releases"
mkdir -p "$repo/latest/download" "$repo/download/v$NEW_VERSION"
printf '{"version":"%s"}\n' "$NEW_VERSION" >"$repo/latest/download/dockering-update.json"
printf 'stub signature\n' >"$repo/latest/download/dockering-update.json.minisig"
cp "$out/new.AppImage" "$repo/download/v$NEW_VERSION/Dockering-x86_64.AppImage"
printf 'stub-public-key\n' >"$fx/public-key.txt"

fails_of() { awk '$2 == "FAIL" { print $1 }' "$1" 2>/dev/null | LC_ALL=C sort | tr '\n' ' ' | sed 's/ $//'; }
skips_of() { awk '$2 == "SKIP" { print $1 }' "$1" 2>/dev/null | LC_ALL=C sort | tr '\n' ' ' | sed 's/ $//'; }
sorted() { tr ' ' '\n' <<<"$1" | grep -v '^$' | LC_ALL=C sort | tr '\n' ' ' | sed 's/ $//'; }

# run_control <entry>: one control, its verdict line in $out/verdict-<name>
run_control() {
  local entry=$1 name fails skips flags results rc got_fails got_skips want_fails want_skips start verdict detail
  IFS='|' read -r name fails skips flags <<<"$entry"
  want_fails=$(sorted "$fails")
  want_skips=$(sorted "$skips")
  results="$out/out-$name/results.txt"
  mkdir -p "$out/home-$name"
  start=$(date +%s)
  env HOME="$out/home-$name" E2E_STUB=1 E2E_STUB_FLAGS="$flags" E2E_STUB_BODY="$here/stub_app.py" \
    E2E_OLD="$out/old.AppImage" E2E_NEW="$out/new.AppImage" E2E_FIXTURE="$fx" E2E_VERSION="$NEW_VERSION" \
    E2E_LEG=extract E2E_OUT="$out/out-$name" E2E_FIRST_CHECK_SECS=2 E2E_WAIT_TREE=10 E2E_WAIT_BUTTON=15 \
    E2E_WAIT_OLD_EXIT="$([ "$name" = old_lingers ] && echo 4 || echo 10)" E2E_WAIT_NEW=10 E2E_ALIVE=1 E2E_SETTLE=2 \
    bash "$here/s1.sh" >"$out/console-$name.txt" 2>&1
  rc=$?
  got_fails=$(fails_of "$results")
  got_skips=$(skips_of "$results")
  if [ ! -f "$results" ]; then
    verdict=FAIL detail="s1.sh wrote no results.txt (exit $rc)"
  elif [ -z "$want_fails" ]; then
    if [ -z "$got_fails" ] && [ -z "$got_skips" ] && [ "$rc" -eq 0 ]; then
      verdict=PASS detail="no check failed, exit 0 ($(($(date +%s) - start))s)"
    else
      verdict=FAIL detail="expected a clean run, FAILs: '${got_fails:-none}', SKIPs: '${got_skips:-none}', exit $rc"
    fi
  elif [ "$got_fails" = "$want_fails" ] && [ "$got_skips" = "$want_skips" ] && [ "$rc" -ne 0 ]; then
    verdict=PASS detail="failed exactly: $got_fails${got_skips:+; skipped: $got_skips} ($(($(date +%s) - start))s)"
  else
    verdict=FAIL detail="expected FAIL of '$want_fails' and SKIP of '${want_skips:-none}', got FAIL '${got_fails:-none}' and SKIP '${got_skips:-none}', exit $rc"
  fi
  printf '%-18s | %s | %s\n' "$name" "$verdict" "$detail" >"$out/verdict-$name"
}

running=0
for entry in "${CONTROLS[@]}"; do
  name=${entry%%|*}
  selected "$name" || continue
  if [ "$running" -ge "$jobs" ]; then wait -n; running=$((running - 1)); fi
  run_control "$entry" &
  running=$((running + 1))
done
wait

passed=0 failed=0
for name in "${names[@]}"; do
  selected "$name" || continue
  if [ -f "$out/verdict-$name" ]; then
    cat "$out/verdict-$name"
    if grep -q ' | PASS | ' "$out/verdict-$name"; then passed=$((passed + 1)); else failed=$((failed + 1)); fi
  else
    printf '%-18s | FAIL | the control produced no verdict\n' "$name"
    failed=$((failed + 1))
  fi
done
echo "== selftest: $passed passed, $failed failed (evidence: $out)"
if [ "$failed" -eq 0 ] && [ -z "$keep" ]; then rm -rf "$out"; fi
[ "$failed" -eq 0 ]
