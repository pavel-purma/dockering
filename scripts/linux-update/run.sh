#!/usr/bin/env bash
# Run the Linux update end-to-end test through Docker (REL-085; plan docs/plan/features/linux-appimage-updates.md §6.5).
#
#   run.sh <image> --old <old.AppImage> --new <new.AppImage> [--scenario s1|s2|s3|s4|all] [--leg extract|fuse|both]
#          [--version X.Y.Z] [--out DIR]
#
# image      the distribution image the app runs in: ubuntu (CI: mirror.gcr.io/library/ubuntu:24.04) or archlinux
#            (ghcr.io/archlinux/archlinux:latest). An AppImage needs at least the glibc of the machine that built it: take the
#            image that built it (a build on an Arch host runs on the Arch image, not on Ubuntu 24.04), or use the CI artifact.
# --old      the OLD AppImage: `updater,update-e2e` and the build-time version 0.0.1 (build-appimages.sh writes Dockering-old-*)
# --new      the NEW AppImage, built exactly like a release (build-appimages.sh writes Dockering-*)
# --scenario s1 (default) the happy path; s2 the new AppImage fails its --version self-test (the manifest claims 99.0.0, which it
#            does not print); s3 a read-only install directory; s4 the predecessor exits late; all = s1, s2, s3, s4 in that order
# --leg      extract: APPIMAGE_EXTRACT_AND_RUN=1; fuse: the AppImage is really mounted (--device /dev/fuse --cap-add SYS_ADMIN
#            --security-opt apparmor=unconfined); both (default): one fresh container per leg for s1, and the extract leg only for
#            s2-s4 (the FUSE mount is what s1 proves; it would only cost CI time there). An explicit extract or fuse applies to
#            every scenario that runs.
# --version  what the NEW AppImage prints for --version, and what the manifest names (default: the workspace version of this tree);
#            s2 names another version in the manifest on purpose (its button says 99.0.0)
# --out      evidence directory (default ./linux-update-out): <scenario>-<leg>/results.txt, requests.jsonl, dockering.log, screenshots, ...
#
# On the HOST: `cargo xtask update-e2e-fixture` writes the throwaway key pair, the signed manifest and a server root that mirrors
# GitHub's paths (a prebuilt xtask binary: `cargo build -p xtask`, found in $DOCKERING_XTASK or target/debug/xtask; xtask never
# runs in the container). In the CONTAINER, as an unprivileged user (root ignores chmod a-w): entry.sh installs the tools, s1.sh
# lays out ~/.local/bin/dockering and its .desktop entry, runs scripts/linux-update/serve.py on 127.0.0.1 and starts the old app
# in real mode with the seam variables (all four scenarios run in s1.sh, picked by E2E_SCENARIO). The inputs are mounted
# read-only; no secret is mounted (the throwaway secret key is deleted after it has signed the manifest); the container needs no
# network access at run time (it installs packages first).
# env:  DOCKERING_XTASK    the xtask binary (default <repo>/target/debug/xtask)
#       E2E_FIRST_CHECK_SECS, E2E_QUIT_DELAY_SECS (s4), E2E_WAIT_TREE, E2E_WAIT_BUTTON, E2E_WAIT_OLD_EXIT, E2E_WAIT_NEW
#                          forwarded to the scenario (seconds)
#       E2E_PULL_DELAY     seconds between pull attempts (default 10)
# Prints one `name PASS|FAIL detail` line per check (the container's), then a line per scenario and leg (`s1 extract PASS <image>`);
# exits 0 only when every one passed, 1 when a check or a step failed, 2 for a usage error.
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: run.sh <image> --old <old.AppImage> --new <new.AppImage> [--scenario s1|s2|s3|s4|all] [--leg extract|fuse|both] [--version X.Y.Z] [--out DIR]
EOF
}
bad() {
  echo "run.sh: $*" >&2
  usage
  exit 2
}

image="" old="" new="" scenario=s1 leg=both version="" out=""
while [ $# -gt 0 ]; do
  case $1 in
    -h | --help)
      usage
      exit 0
      ;;
    --old | --new | --scenario | --leg | --version | --out)
      [ $# -ge 2 ] || bad "$1 needs a value"
      case $1 in
        --old) old=$2 ;;
        --new) new=$2 ;;
        --scenario) scenario=$2 ;;
        --leg) leg=$2 ;;
        --version) version=$2 ;;
        --out) out=$2 ;;
      esac
      shift 2
      ;;
    --old=* | --new=* | --scenario=* | --leg=* | --version=* | --out=*)
      key=${1%%=*}
      value=${1#*=}
      case $key in
        --old) old=$value ;;
        --new) new=$value ;;
        --scenario) scenario=$value ;;
        --leg) leg=$value ;;
        --version) version=$value ;;
        --out) out=$value ;;
      esac
      shift
      ;;
    -*) bad "unknown option '$1'" ;;
    *)
      [ -z "$image" ] || bad "unexpected argument '$1' (the image is already '$image')"
      image=$1
      shift
      ;;
  esac
done

image_re='^[A-Za-z0-9][A-Za-z0-9._/:@-]*$'
version_re='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'
secs_re='^[0-9]{1,4}$'
[ -n "$image" ] || bad "the image is required"
[[ $image =~ $image_re ]] || bad "unexpected image reference '$image'"
[ -n "$old" ] || bad "--old is required"
[ -n "$new" ] || bad "--new is required"
case $scenario in s1 | s2 | s3 | s4 | all) ;; *) bad "--scenario must be s1, s2, s3, s4 or all, not '$scenario'" ;; esac
case $leg in extract | fuse | both) ;; *) bad "--leg must be extract, fuse or both, not '$leg'" ;; esac
[ -z "$version" ] || [[ $version =~ $version_re ]] || bad "--version must be X.Y.Z, not '$version'"
for name in E2E_FIRST_CHECK_SECS E2E_QUIT_DELAY_SECS E2E_WAIT_TREE E2E_WAIT_BUTTON E2E_WAIT_OLD_EXIT E2E_WAIT_NEW; do
  [ -z "${!name:-}" ] || [[ ${!name} =~ $secs_re ]] || bad "$name must be a number of seconds, not '${!name}'"
done
for file in "$old" "$new"; do
  [ -f "$file" ] || bad "no such file: $file"
  [ -s "$file" ] || bad "$file is empty"
  [ -r "$file" ] || bad "$file is not readable"
done
[ "$(cd "$(dirname "$old")" && pwd)/$(basename "$old")" != "$(cd "$(dirname "$new")" && pwd)/$(basename "$new")" ] || bad "--old and --new are the same file"

# Git Bash on Windows must neither rewrite the container side of a mount nor hand docker a /c/... path
export MSYS_NO_PATHCONV=1
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$here/../.." && pwd)
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d ' ' -f 1; else shasum -a 256 "$1" | cut -d ' ' -f 1; fi
}
[ "$(sha256 "$old")" != "$(sha256 "$new")" ] || bad "--old and --new have the same content: the update would be a no-op"
old=$(cd "$(dirname "$old")" && pwd)/$(basename "$old")
new=$(cd "$(dirname "$new")" && pwd)/$(basename "$new")

if [ -z "$version" ]; then
  version=$(awk '/^\[workspace\.package\]/ { in_table = 1; next } /^\[/ { in_table = 0 } in_table && $1 == "version" { gsub(/"/, "", $3); print $3; exit }' "$repo/Cargo.toml")
  [[ $version =~ $version_re ]] || { echo "run.sh: no workspace version in $repo/Cargo.toml (found '$version'); pass --version" >&2; exit 1; }
fi

xtask=${DOCKERING_XTASK:-}
if [ -z "$xtask" ]; then
  for candidate in "$repo/target/debug/xtask" "$repo/target/release/xtask" "${CARGO_TARGET_DIR:+$CARGO_TARGET_DIR/debug/xtask}"; do
    if [ -n "$candidate" ] && [ -x "$candidate" ]; then
      xtask=$candidate
      break
    fi
  done
fi
if [ -z "$xtask" ] || [ ! -x "$xtask" ]; then echo "run.sh: no xtask binary: run 'cargo build -p xtask' or set DOCKERING_XTASK" >&2; exit 1; fi
command -v docker >/dev/null 2>&1 || { echo "run.sh: docker is not installed" >&2; exit 1; }

[ -n "$out" ] || out=$PWD/linux-update-out
mkdir -p "$out" || { echo "run.sh: cannot create $out" >&2; exit 1; }
out=$(cd "$out" && pwd)

# pull (the registry's copy is the one under test); a copy that is already here is the fallback
delay=${E2E_PULL_DELAY:-10}
[[ $delay =~ ^[0-9]+$ ]] || delay=10
pulled=""
for try in 1 2 3; do
  if docker pull --quiet "$image" >/dev/null; then
    pulled=1
    break
  fi
  echo "run.sh: could not pull $image (try $try of 3)" >&2
  if [ "$try" -lt 3 ]; then sleep "$delay"; fi
done
if [ -z "$pulled" ]; then
  docker image inspect "$image" >/dev/null 2>&1 || { echo "run.sh: $image could not be pulled and is not available locally" >&2; exit 1; }
  echo "run.sh: using the local copy of $image" >&2
fi
# the repo digest of the registry the image came from (a local image can carry several)
name=${image%@*}
[[ ${name##*/} == *:* ]] && name=${name%:*}
digests=$(docker image inspect "$image" --format '{{range .RepoDigests}}{{println .}}{{end}}' 2>/dev/null || true)
digest=$(grep -F -m 1 "$name@" <<<"$digests" || true)
[ -n "$digest" ] || digest=$(head -n 1 <<<"$digests")
printf 'image=%s\ndigest=%s\nold=%s sha256 %s\nnew=%s sha256 %s\nversion=%s\n' "$image" "${digest:-unknown}" \
  "$old" "$(sha256 "$old")" "$new" "$(sha256 "$new")" "$version" >"$out/image.txt"
echo "run.sh: $scenario on $image (${digest:-no repo digest}); old $(sha256 "$old" | cut -c1-12), new $(sha256 "$new" | cut -c1-12), version $version" >&2

scenarios=("$scenario")
[ "$scenario" != all ] || scenarios=(s1 s2 s3 s4)
# s2 names a version in the manifest that the NEW AppImage does not print, so that its --version self-test fails
claimed=99.0.0
[ "$version" != "$claimed" ] || claimed=99.0.1

# the legs of a scenario: `both` is s1's two legs and the extract leg of the others; an explicit leg applies to every scenario
legs_of() { # <scenario>
  if [ "$leg" != both ]; then echo "$leg"; elif [ "$1" = s1 ]; then echo "extract fuse"; else echo extract; fi
}

summary=()
status=0
fixture=""
# shellcheck disable=SC2317,SC2329 # runs from the EXIT trap (older shellchecks say SC2317, newer ones SC2329)
cleanup() {
  if [ -n "$fixture" ]; then rm -rf "$fixture"; fi
}
trap cleanup EXIT

run_leg() { # <scenario> <leg>
  # (an empty array is `unbound` under `set -u` in bash 3.2, macOS's: expanded with the `${name[@]+...}` form)
  local sc=$1 lg=$2 leg_out="$out/$1-$2" rc=0 flags=() env_args=() fixture_args=() var shown=$version
  fixture="$out/.fixture-$sc-$lg"
  rm -rf "$fixture" "$leg_out"
  mkdir -p "$leg_out"
  # s2: the manifest, its URL and the button name the claimed version while the file still prints $version
  if [ "$sc" = s2 ]; then
    shown=$claimed
    fixture_args=(--claimed-version "$claimed")
    env_args+=(-e "E2E_NEW_VERSION=$version")
  fi
  # the fixture is written on the host and never needs the container: a key pair, the signed manifest, the server root
  if ! (cd "$repo" && "$xtask" update-e2e-fixture --out "$fixture" --appimage "$new" --version "$version" ${fixture_args[@]+"${fixture_args[@]}"}) >"$leg_out/xtask.log" 2>&1; then
    echo "run.sh: cargo xtask update-e2e-fixture failed:" >&2
    tail -n 5 "$leg_out/xtask.log" >&2
    return 1
  fi
  rm -f "$fixture/e2e.key" # throwaway, and it has done its one job; it is never mounted or kept
  mkdir -p "$leg_out/fixture"
  cp "$fixture/dockering-update.json" "$fixture/dockering-update.json.minisig" "$fixture/public-key.txt" "$leg_out/fixture/"
  if [ "$lg" = fuse ]; then flags=(--device /dev/fuse --cap-add SYS_ADMIN --security-opt apparmor=unconfined); fi
  for var in E2E_FIRST_CHECK_SECS E2E_QUIT_DELAY_SECS E2E_WAIT_TREE E2E_WAIT_BUTTON E2E_WAIT_OLD_EXIT E2E_WAIT_NEW; do
    if [ -n "${!var:-}" ]; then env_args+=(-e "$var"); fi
  done
  echo "run.sh: $sc, $lg leg" >&2
  if docker run --rm --name "linux-update-$sc-$lg-$$" --shm-size=512m ${flags[@]+"${flags[@]}"} \
    -v "$here:/scripts:ro" -v "$old:/in/old.AppImage:ro" -v "$new:/in/new.AppImage:ro" \
    -v "$fixture/root:/fixture/root:ro" -v "$fixture/public-key.txt:/fixture/public-key.txt:ro" -v "$leg_out:/out" \
    -e "E2E_SCENARIO=$sc" -e "E2E_LEG=$lg" -e "E2E_VERSION=$shown" -e "HOST_UID=$(id -u)" -e "HOST_GID=$(id -g)" ${env_args[@]+"${env_args[@]}"} \
    "$image" bash /scripts/entry.sh "$sc" 2>&1 | tee "$leg_out/leg.log"; then
    rc=0
  else
    rc=${PIPESTATUS[0]}
  fi
  rm -rf "$fixture"
  fixture=""
  if [ ! -f "$leg_out/results.txt" ]; then
    echo "run.sh: $sc $lg leg wrote no results.txt (container exit $rc)" >&2
    return 1
  fi
  # the container's exit status and its results must agree: a FAIL line can't hide behind a zero exit
  if [ "$rc" -eq 0 ] && awk '$2 == "FAIL" { found = 1 } END { exit !found }' "$leg_out/results.txt"; then rc=1; fi
  return "$rc"
}

for sc in "${scenarios[@]}"; do
  read -ra scenario_legs <<<"$(legs_of "$sc")"
  for lg in "${scenario_legs[@]}"; do
    if run_leg "$sc" "$lg"; then
      summary+=("$sc $lg PASS $image")
    else
      summary+=("$sc $lg FAIL $image")
      status=1
    fi
  done
done
echo "== linux-update e2e, evidence in $out"
printf '%s\n' "${summary[@]}"
exit "$status"
