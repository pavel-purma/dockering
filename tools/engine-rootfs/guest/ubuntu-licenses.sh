#!/bin/sh
# Runs INSIDE the Ubuntu image (dash, via chroot): prints the licence notices of the installed packages
# to stdout (REL-002). A summary (package, version, licence names from /usr/share/doc/<pkg>/copyright),
# then the full copyright files of the Docker packages and of the components Dockering adds.
set -eu

echo "Dockering engine image (Ubuntu): third-party licence notices"
echo "Generated from dpkg; the exact versions are in /etc/dockering-engine-manifest.txt."
echo
echo "== Installed packages and the licences their copyright files name =="
dpkg-query -W -f='${binary:Package}\t${Version}\n' | sort | while IFS="$(printf '\t')" read -r pkg ver; do
  name=${pkg%%:*}
  file=/usr/share/doc/$name/copyright
  if [ -f "$file" ]; then
    lic=$(sed -n 's/^License: *//p' "$file" | sed 's/ *$//' | grep -v '^$' | sort -u | tr '\n' ';' | sed 's/;$//; s/;/; /g')
    [ -n "$lic" ] || lic="(see the copyright file)"
  else
    lic="(no copyright file in the image)"
  fi
  printf '%s %s: %s\n' "$pkg" "$ver" "$lic"
done
echo
echo "== Full notices of the packages Dockering adds =="
for pkg in docker-ce docker-ce-cli containerd.io; do
  for file in "/usr/share/doc/$pkg/copyright" "/usr/share/doc/$pkg/LICENSE" "/usr/share/doc/$pkg/NOTICE" "/usr/share/doc/$pkg/"THIRD-PARTY-LICENSES*; do
    if [ -f "$file" ]; then
      echo
      echo "-- $file --"
      cat -- "$file"
    fi
  done
done
echo
echo "== docker-ce and docker-ce-cli =="
echo "Docker's packages ship no copyright file. Docker Engine (moby/moby) and the Docker CLI (docker/cli) are licensed"
echo "under the Apache License 2.0 (https://github.com/moby/moby/blob/master/LICENSE, https://github.com/docker/cli/blob/master/LICENSE)."
echo "Their notices are in https://github.com/moby/moby/blob/master/NOTICE and https://github.com/docker/cli/blob/master/NOTICE."
if [ -f /usr/share/common-licenses/Apache-2.0 ]; then
  echo
  echo "-- /usr/share/common-licenses/Apache-2.0 --"
  cat -- /usr/share/common-licenses/Apache-2.0
fi
echo
echo "== Source code =="
echo "Ubuntu packages: 'apt-get source <package>' on a system with deb-src enabled, or https://archive.ubuntu.com/ubuntu/pool/."
echo "Docker Engine: https://github.com/moby/moby, Docker CLI: https://github.com/docker/cli, containerd: https://github.com/containerd/containerd, runc: https://github.com/opencontainers/runc."
