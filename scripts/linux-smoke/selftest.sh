#!/usr/bin/env bash
# Run the self-test of the Linux smoke test (plan task 3; REL-072..075): the harness must fail when it should.
# Pulls an Ubuntu image, installs the .deb in a fresh container and runs run.sh against deliberately broken inputs
# (selftest-entry.sh, with stub-app.sh as the defective application); each control states which checks must FAIL.
# Plan: docs/plan/features/linux-install-smoke-test.md. Used by linux-smoke.yml (job selftest) and by developers.
#   selftest.sh <pkg-dir> <out-dir> <version> [<image>...]
# pkg-dir holds the packages and packages.sha256 (fetch.sh writes them); out-dir collects the evidence; the images
# default to the Ubuntu 24.04 mirror and Docker Hub as its fallback.
# env:  SMOKE_SELFTEST_ONLY  space-separated control names to run (default: all; names are printed by a full run)
#       SMOKE_PULL_DELAY     seconds between pull attempts (default 10)
# Exits with the status of selftest-entry.sh: 0 only when every control behaved.
set -u
usage() { echo "usage: selftest.sh <pkg-dir> <out-dir> <version> [<image>...]" >&2; exit 2; }
[ $# -ge 3 ] || usage
pkg=$1 out=$2 version=$3
shift 3
images=("$@")
[ ${#images[@]} -gt 0 ] || images=(mirror.gcr.io/library/ubuntu:24.04 docker.io/library/ubuntu:24.04)
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
export SMOKE_ENTRY=selftest-entry.sh
exec bash "$here/leg.sh" selftest deb "$pkg" "$out" "$version" "${images[@]}"
