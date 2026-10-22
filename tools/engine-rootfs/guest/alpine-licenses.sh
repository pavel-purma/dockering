#!/bin/sh
# Runs INSIDE the Alpine image (busybox sh, via chroot): prints the licence notices of the installed
# packages to stdout (REL-002): every package with its SPDX licence expression, then any licence texts
# the packages ship under /usr/share/licenses (Alpine packages mostly ship none), then where the
# corresponding source code is.
set -eu

echo "Dockering engine image (Alpine): third-party licence notices"
echo "Generated from the installed package database; versions are in /etc/dockering-engine-manifest.txt."
echo
echo "== Installed packages and their licences (SPDX expressions from the apk database) =="
apk list --installed | sed -n 's/^\([^ ]*\) [^ ]* {\([^}]*\)} (\([^)]*\)).*/\1 (origin \2): \3/p' | sort -u
echo
echo "== Licence texts shipped by the packages (/usr/share/licenses) =="
if [ -d /usr/share/licenses ] && [ -n "$(find /usr/share/licenses -type f | head -n 1)" ]; then
  find /usr/share/licenses -type f | sort | while IFS= read -r f; do
    echo
    echo "-- $f --"
    cat -- "$f"
  done
else
  echo "(none: the packages ship no separate licence files; the full texts of the SPDX identifiers above are at https://spdx.org/licenses/)"
fi
echo
echo "== Source code =="
echo "Alpine packages are built from the recipes and sources in https://gitlab.alpinelinux.org/alpine/aports (branch 3.24-stable)."
echo "Docker Engine: https://github.com/moby/moby, Docker CLI: https://github.com/docker/cli, containerd: https://github.com/containerd/containerd, runc: https://github.com/opencontainers/runc."
