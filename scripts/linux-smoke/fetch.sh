#!/usr/bin/env bash
# Download the Linux or macOS packages of a release, or of a release run that has not been published yet, and verify them
# (REL-071): exactly the expected SHA256SUMS entries, matching checksums, and build provenance pinned to release.yml.
# Plan: docs/plan/features/linux-install-smoke-test.md
#   fetch.sh <tag|latest> <dest>             the published release
#   fetch.sh run:<run-id> <dest>             the `release-review-*` bundle of a Release workflow run (before publication)
# env:  GITHUB_REPOSITORY  owner/name (required)
#       GH_TOKEN           token for gh (not needed when gh is logged in)
#       ARCH               package architecture (default x86_64; aarch64 for arm64)
#       PLATFORM           linux (default: .deb, .AppImage, .tar.gz) or macos (.dmg)
# stdout: only `tag=<tag>` and `version=<version>`, for $GITHUB_OUTPUT; everything else goes to stderr.
set -euo pipefail
exec 3>&1 1>&2

fail() { printf '::error::fetch.sh: %s\n' "$*"; exit 1; }

[ $# -eq 2 ] || { echo "usage: fetch.sh <tag|latest|run:<run-id>> <dest>"; exit 2; }
source_arg=$1
dest=$2
repo=${GITHUB_REPOSITORY:-}
arch=${ARCH:-x86_64}
platform=${PLATFORM:-linux}
repo_re='^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$'
arch_re='^[A-Za-z0-9_]+$'
tag_re='^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'
run_re='^run:[0-9]+$'
sha_re='^[0-9a-f]{40}$'
[ -n "$repo" ] || fail "GITHUB_REPOSITORY is not set (owner/name)"
[[ $repo =~ $repo_re ]] || fail "GITHUB_REPOSITORY is not owner/name"
[[ $arch =~ $arch_re ]] || fail "ARCH has unexpected characters"
case "$platform" in
linux) files=("Dockering-$arch.deb" "Dockering-$arch.AppImage" "Dockering-$arch.tar.gz") ;;
macos) files=("Dockering-$arch.dmg") ;;
*) fail "PLATFORM must be linux or macos" ;;
esac
count=${#files[@]}

run_id=""
tag=$source_arg
if [[ $source_arg =~ $run_re ]]; then
  run_id=${source_arg#run:}
  info=$(gh run view "$run_id" --repo "$repo" --json workflowDatabaseId,headBranch,headSha,event,conclusion \
    --jq '[.workflowDatabaseId, .event, .conclusion, .headBranch, .headSha] | @tsv') || fail "cannot read run $run_id"
  IFS=$'\t' read -r run_workflow run_event run_conclusion tag run_sha <<<"$info"
  release_workflow=$(gh api "repos/$repo/actions/workflows/release.yml" --jq .id) || fail "cannot read the release.yml workflow"
  [ "$run_workflow" = "$release_workflow" ] || fail "run $run_id is not a Release workflow run"
  [ "$run_event" = push ] || fail "run $run_id was not started by a tag push (event $run_event)"
  [ "$run_conclusion" = success ] || fail "run $run_id did not succeed ($run_conclusion)"
  # the workflow file a tag run executes comes from the tagged commit, so a tag on a side branch could carry its own release.yml
  [[ $run_sha =~ $sha_re ]] || fail "run $run_id has no usable head commit"
  on_main=$(gh api "repos/$repo/compare/main...$run_sha" --jq .status) || fail "cannot compare the commit of run $run_id with main"
  case "$on_main" in
  identical | behind) ;;
  *) fail "the commit of run $run_id is not on main ($on_main)" ;;
  esac
elif [ "$source_arg" = latest ]; then
  tag=$(gh release view --repo "$repo" --json tagName --jq .tagName) || fail "cannot resolve the latest release of $repo"
fi
[[ $tag =~ $tag_re ]] || fail "unexpected release tag $(printf '%q' "$tag")"
echo "::notice::testing $tag${run_id:+ (run $run_id, before publication)}"
echo "fetch.sh: $repo $tag ($platform $arch)"

mkdir -p "$dest"
for f in "${files[@]}" SHA256SUMS packages.sha256; do rm -f -- "$dest/$f"; done
if [ -n "$run_id" ]; then
  version=${tag#v}
  gh run download "$run_id" --repo "$repo" --name "release-review-$version" --dir "$dest/.bundle" ||
    fail "cannot download release-review-$version of run $run_id (expired or never uploaded?)"
  for f in "${files[@]}" SHA256SUMS; do
    [ -f "$dest/.bundle/$f" ] || fail "release-review-$version has no file $f"
    mv -f -- "$dest/.bundle/$f" "$dest/$f"
  done
  rm -rf -- "$dest/.bundle"
else
  patterns=()
  for f in "${files[@]}"; do patterns+=(--pattern "$f"); done
  gh release download "$tag" --repo "$repo" --dir "$dest" "${patterns[@]}" --pattern SHA256SUMS ||
    fail "cannot download the assets of $tag"
  for f in "${files[@]}" SHA256SUMS; do [ -f "$dest/$f" ] || fail "$tag has no asset $f"; done
fi

# the package lines, and nothing else, go to packages.sha256
case "$platform" in linux) ext_re='(deb|AppImage|tar\.gz)' ;; *) ext_re='dmg' ;; esac
sums_re="^[0-9a-f]{64} [ *]Dockering-${arch}\\.${ext_re}\$"
grep -E "$sums_re" "$dest/SHA256SUMS" >"$dest/packages.sha256" || true
if [ "$platform" = linux ]; then expected="three $arch packages"; else expected="one macOS $arch package"; fi
[ "$(wc -l <"$dest/packages.sha256" | tr -d ' ')" -eq "$count" ] || fail "SHA256SUMS does not list exactly $expected"
for f in "${files[@]}"; do
  [ "$(awk -v f="$f" 'substr($0, 67) == f' "$dest/packages.sha256" | wc -l | tr -d ' ')" -eq 1 ] ||
    fail "SHA256SUMS does not list $f exactly once"
done
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$dest" && sha256sum -c --strict packages.sha256) || fail "checksum mismatch"
else
  (cd "$dest" && shasum -a 256 -c packages.sha256) || fail "checksum mismatch"
fi

for f in "${files[@]}"; do
  gh attestation verify "$dest/$f" --repo "$repo" \
    --signer-workflow "$repo/.github/workflows/release.yml" \
    --source-ref "refs/tags/$tag" --deny-self-hosted-runners || fail "no valid build provenance for $f"
  echo "fetch.sh: build provenance verified for $f"
done

printf 'tag=%s\nversion=%s\n' "$tag" "${tag#v}" >&3
