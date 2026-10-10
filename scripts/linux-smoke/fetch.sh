#!/usr/bin/env bash
# Download the published Linux packages of a release and verify them (REL-071): exactly three SHA256SUMS
# entries, matching checksums, and build provenance pinned to release.yml and the tag.
# Plan: docs/plan/features/linux-install-smoke-test.md
#   fetch.sh <tag|latest> <dest>
# env:  GITHUB_REPOSITORY  owner/name (required)
#       GH_TOKEN           token for gh (not needed when gh is logged in)
#       ARCH               package architecture (default x86_64)
# stdout: only `tag=<tag>` and `version=<version>`, for $GITHUB_OUTPUT; everything else goes to stderr.
set -euo pipefail
exec 3>&1 1>&2

fail() { printf '::error::fetch.sh: %s\n' "$*"; exit 1; }

[ $# -eq 2 ] || { echo "usage: fetch.sh <tag|latest> <dest>"; exit 2; }
tag=$1
dest=$2
repo=${GITHUB_REPOSITORY:-}
arch=${ARCH:-x86_64}
repo_re='^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$'
arch_re='^[A-Za-z0-9_]+$'
tag_re='^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'
[ -n "$repo" ] || fail "GITHUB_REPOSITORY is not set (owner/name)"
[[ $repo =~ $repo_re ]] || fail "GITHUB_REPOSITORY is not owner/name"
[[ $arch =~ $arch_re ]] || fail "ARCH has unexpected characters"

if [ "$tag" = latest ]; then
  tag=$(gh release view --repo "$repo" --json tagName --jq .tagName) || fail "cannot resolve the latest release of $repo"
fi
[[ $tag =~ $tag_re ]] || fail "unexpected release tag $(printf '%q' "$tag")"
echo "::notice::testing $tag"
echo "fetch.sh: $repo $tag ($arch)"

files=("Dockering-$arch.deb" "Dockering-$arch.AppImage" "Dockering-$arch.tar.gz")
mkdir -p "$dest"
for f in "${files[@]}" SHA256SUMS packages.sha256; do rm -f -- "$dest/$f"; done
gh release download "$tag" --repo "$repo" --dir "$dest" \
  --pattern "${files[0]}" --pattern "${files[1]}" --pattern "${files[2]}" --pattern SHA256SUMS ||
  fail "cannot download the assets of $tag"
for f in "${files[@]}" SHA256SUMS; do [ -f "$dest/$f" ] || fail "$tag has no asset $f"; done

# the three package lines, and nothing else, go to packages.sha256
sums_re="^[0-9a-f]{64} [ *]Dockering-${arch}\\.(deb|AppImage|tar\\.gz)\$"
grep -E "$sums_re" "$dest/SHA256SUMS" >"$dest/packages.sha256" || true
[ "$(wc -l <"$dest/packages.sha256" | tr -d ' ')" -eq 3 ] || fail "SHA256SUMS does not list exactly three $arch packages"
for f in "${files[@]}"; do
  [ "$(awk -v f="$f" 'substr($0, 67) == f' "$dest/packages.sha256" | wc -l | tr -d ' ')" -eq 1 ] ||
    fail "SHA256SUMS does not list $f exactly once"
done
(cd "$dest" && sha256sum -c packages.sha256) || fail "checksum mismatch"

for f in "${files[@]}"; do
  gh attestation verify "$dest/$f" --repo "$repo" \
    --signer-workflow "$repo/.github/workflows/release.yml" \
    --source-ref "refs/tags/$tag" --deny-self-hosted-runners || fail "no valid build provenance for $f"
  echo "fetch.sh: build provenance verified for $f"
done

printf 'tag=%s\nversion=%s\n' "$tag" "${tag#v}" >&3
