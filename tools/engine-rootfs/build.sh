#!/usr/bin/env bash
# Build one engine root filesystem (REL-088, ENG-152, ENG-164).
#
#   build.sh <alpine|ubuntu> <x86_64|aarch64> <outdir>
#
# Runs as root or with passwordless sudo on a Linux host of the SAME architecture (GitHub's
# ubuntu-24.04 for x86_64, ubuntu-24.04-arm for aarch64): the image is assembled in a chroot, so
# a foreign architecture would need qemu-user (ENGINE_ROOTFS_ALLOW_CROSS=1 skips the check; CI does not).
# All pins (base URL, size, SHA-256, package versions) come from tools/engine-rootfs/pins.toml.
#
# Outputs in <outdir>:
#   dockering-engine-<image>-<arch>.tar.xz       the asset (WSL imports it directly)
#   dockering-engine-<image>-<arch>.tar.xz.sha256
#   dockering-engine-<image>-<arch>.json         name, size, sha256, url, base_*, packages, tar_sha256
#   dockering-engine-<image>-<arch>.manifest.txt copy of /etc/dockering-engine-manifest.txt
#   LICENSES-<image>-<arch>.txt                  licence notices of the contents (REL-002)
#
# Environment: ENGINE_ROOTFS_TAG (release tag used in the JSON url, default rootfs-0),
# ENGINE_ROOTFS_KEEP_WORK=1 keeps the scratch directory. No secrets are read or needed.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
readonly HERE

die() { printf 'build.sh: error: %s\n' "$*" >&2; exit 1; }
note() { printf 'build.sh: %s\n' "$*" >&2; }

[ $# -eq 3 ] || die "usage: build.sh <alpine|ubuntu> <x86_64|aarch64> <outdir>"
IMAGE=$1
ARCH=$2
case $IMAGE in alpine | ubuntu) ;; *) die "unknown image '$IMAGE' (alpine or ubuntu)" ;; esac
case $ARCH in x86_64 | aarch64) ;; *) die "unknown architecture '$ARCH' (x86_64 or aarch64)" ;; esac
mkdir -p -- "$3"
OUT=$(cd -- "$3" && pwd)
TAG=${ENGINE_ROOTFS_TAG:-rootfs-0}
[[ $TAG =~ ^rootfs-(0|[1-9][0-9]*)$ ]] || die "ENGINE_ROOTFS_TAG must look like rootfs-<n>"

[ "$(uname -s)" = Linux ] || die "needs a Linux host"
if [ "$(uname -m)" != "$ARCH" ] && [ "${ENGINE_ROOTFS_ALLOW_CROSS:-0}" != 1 ]; then
  die "host is $(uname -m), image is $ARCH: build on a native runner (or set ENGINE_ROOTFS_ALLOW_CROSS=1 with qemu-user-static)"
fi

SUDO=()
if [ "$(id -u)" -ne 0 ]; then SUDO=(sudo); fi

for tool in python3 curl xz tar sha256sum chroot mount umount awk grep sed gpg; do
  command -v "$tool" > /dev/null 2>&1 || die "missing tool: $tool"
done

pin() { python3 -B -I "$HERE/pins.py" "$@"; }

NAME=dockering-engine-$IMAGE-$ARCH
SOURCE_DATE_EPOCH=$(pin get build.source_date_epoch)
export SOURCE_DATE_EPOCH
BASE_URL=$(pin arch-get "$IMAGE" "$ARCH" base_url)
BASE_SIZE=$(pin arch-get "$IMAGE" "$ARCH" base_size)
BASE_SHA256=$(pin arch-get "$IMAGE" "$ARCH" base_sha256)
UPGRADE_BASE=$(pin get "images.$IMAGE.upgrade_base")

WORK=$(mktemp -d "${TMPDIR:-/tmp}/engine-rootfs.XXXXXX")
ROOT=$WORK/root
MOUNTS=()
DNS_SAVED=0

umount_all() {
  local i
  for ((i = ${#MOUNTS[@]} - 1; i >= 0; i--)); do
    "${SUDO[@]}" umount "${MOUNTS[i]}" || return 1
  done
  MOUNTS=()
}

cleanup() {
  local status=$?
  set +e
  if ! umount_all; then
    note "could not unmount ${MOUNTS[*]}; leaving $WORK in place"
    exit "$status"
  fi
  if grep -qF -- " $WORK/" /proc/self/mountinfo; then
    note "mounts remain under $WORK; leaving it in place"
    exit "$status"
  fi
  if [ "${ENGINE_ROOTFS_KEEP_WORK:-0}" = 1 ]; then
    note "kept $WORK"
  else
    "${SUDO[@]}" rm -rf --one-file-system -- "$WORK"
  fi
  exit "$status"
}
trap cleanup EXIT

# --- verified download: nothing is unpacked before the SHA-256 matches ----------------------------
download_verified() {
  local url=$1 size=$2 sha=$3 dest=$4 got
  note "downloading $url"
  curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
    --retry 3 --retry-delay 5 --connect-timeout 30 --max-filesize "$size" -o "$dest" -- "$url"
  got=$(stat -c %s -- "$dest")
  [ "$got" = "$size" ] || { rm -f -- "$dest"; die "size of $url is $got, pinned $size"; }
  got=$(sha256sum -- "$dest" | cut -d' ' -f1)
  [ "$got" = "$sha" ] || { rm -f -- "$dest"; die "SHA-256 of $url is $got, pinned $sha"; }
  note "verified $(basename -- "$dest") ($size bytes, sha256 $sha)"
}

unpack_base() {
  mkdir -p -- "$ROOT"
  "${SUDO[@]}" tar --extract --preserve-permissions --same-owner --numeric-owner --xattrs \
    --xattrs-include='security.capability' --file "$1" --directory "$ROOT"
}

mount_chroot() {
  "${SUDO[@]}" mkdir -p "$ROOT/proc" "$ROOT/dev"
  "${SUDO[@]}" mount -t proc proc "$ROOT/proc"
  MOUNTS+=("$ROOT/proc")
  "${SUDO[@]}" mount --bind /dev "$ROOT/dev"
  MOUNTS+=("$ROOT/dev")
}

prepare_dns() {
  if [ -e "$ROOT/etc/resolv.conf" ] || [ -L "$ROOT/etc/resolv.conf" ]; then
    "${SUDO[@]}" mv -- "$ROOT/etc/resolv.conf" "$ROOT/etc/resolv.conf.dockering-orig"
    DNS_SAVED=1
  fi
  "${SUDO[@]}" cp --dereference /etc/resolv.conf "$ROOT/etc/resolv.conf"
}

restore_dns() {
  "${SUDO[@]}" rm -f -- "$ROOT/etc/resolv.conf"
  if [ "$DNS_SAVED" = 1 ]; then
    "${SUDO[@]}" mv -- "$ROOT/etc/resolv.conf.dockering-orig" "$ROOT/etc/resolv.conf"
  fi
}

# Run a command inside the image with a clean environment.
in_root() {
  "${SUDO[@]}" env -i HOME=/root LANG=C.UTF-8 DEBIAN_FRONTEND=noninteractive \
    PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin chroot "$ROOT" "$@"
}

put_file() { # put_file <mode> <path-in-image>   (content on stdin)
  "${SUDO[@]}" tee -- "$ROOT$2" > /dev/null
  "${SUDO[@]}" chmod "$1" -- "$ROOT$2"
}

# Edit an INI file the way WSL reads it: make sure [section] has `key = value`, keeping every other line.
# A new key goes after the section's last non-blank line.
ini_set() { # ini_set <file> <section> <key> <value>
  local file=$1 section=$2 key=$3 value=$4 tmp
  tmp=$(mktemp "$WORK/ini.XXXXXX")
  "${SUDO[@]}" cat -- "$file" 2> /dev/null | awk -v sec="$section" -v key="$key" -v val="$value" '
    function blanks() { while (pending > 0) { print ""; pending-- } }
    function flush() { if (insec && !done) { print key " = " val; done = 1 } blanks() }
    /^\[.*\][[:space:]]*$/ {
      flush()
      insec = ($0 == "[" sec "]")
      if (insec) seen = 1
      done = 0
      print; next
    }
    /^[[:space:]]*$/ { pending++; next }
    {
      blanks()
      if (insec && $0 ~ "^[[:space:]]*" key "[[:space:]]*=") { if (!done) { print key " = " val; done = 1 } next }
      print
    }
    END {
      flush()
      if (!seen) { if (NR) print ""; print "[" sec "]"; print key " = " val }
    }' > "$tmp"
  "${SUDO[@]}" tee -- "$file" < "$tmp" > /dev/null
  rm -f -- "$tmp"
}

# The ELF machine type of an installed binary (little-endian e_machine at offset 18): 3e = x86-64, b7 = AArch64.
# The arm64 cells cannot be smoke-tested on a hosted runner, so this at least proves the right packages went in.
check_elf_arch() { # check_elf_arch <path-in-image>
  local want got
  case $ARCH in x86_64) want="3e 00" ;; aarch64) want="b7 00" ;; esac
  got=$("${SUDO[@]}" od -An -t x1 -j 18 -N 2 -- "$ROOT$1" | tr -s ' ' | sed 's/^ //; s/ $//')
  [ "$got" = "$want" ] || die "$1 has ELF machine '$got', expected '$want' for $ARCH"
}

# --- 1. base -------------------------------------------------------------------------------------
BASE_FILE=$WORK/$(basename -- "$BASE_URL")
download_verified "$BASE_URL" "$BASE_SIZE" "$BASE_SHA256" "$BASE_FILE"
unpack_base "$BASE_FILE"
rm -f -- "$BASE_FILE"

# Files that package triggers rewrite with build-time values (a fresh machine id, inode numbers, timestamps) are put
# back as the verified base shipped them, so that the same inputs give the same tar. /etc/machine-id stays empty:
# systemd gives every imported distro its own id at first boot.
KEEP_PATHS=(etc/machine-id var/lib/command-not-found/commands.db.metadata var/cache/man/index.db)
for f in "${KEEP_PATHS[@]}"; do
  if [ -e "$ROOT/$f" ]; then
    mkdir -p -- "$WORK/keep/$(dirname -- "$f")"
    "${SUDO[@]}" cp -a -- "$ROOT/$f" "$WORK/keep/$f"
  fi
done
restore_kept() {
  local f
  for f in "${KEEP_PATHS[@]}"; do
    if [ -e "$WORK/keep/$f" ]; then
      "${SUDO[@]}" rm -f -- "$ROOT/$f"
      "${SUDO[@]}" cp -a -- "$WORK/keep/$f" "$ROOT/$f"
    fi
  done
}

mount_chroot
prepare_dns

MANIFEST_TMP=$WORK/manifest.txt
LICENSES_TMP=$WORK/licenses.txt

write_manifest_head() {
  {
    echo "dockering-engine-manifest: 1"
    echo "image: $IMAGE"
    echo "arch: $ARCH"
    echo "base_url: $BASE_URL"
    echo "base_sha256: $BASE_SHA256"
    echo "source_date_epoch: $SOURCE_DATE_EPOCH"
  } > "$MANIFEST_TMP"
}

# --- 2. per-image recipe -------------------------------------------------------------------------
case $IMAGE in
  alpine)
    mapfile -t APK_PACKAGES < <(pin get images.alpine.apk_packages)
    in_root /sbin/apk add --no-cache "${APK_PACKAGES[@]}"
    if [ "$UPGRADE_BASE" = true ]; then in_root /sbin/apk upgrade --no-cache; fi

    # The minirootfs has no OpenRC, so the boot command is the only start mechanism. The boot command has no
    # inherited environment (spike W-3.3): without the explicit PATH dockerd cannot find iptables or runc.
    # --userland-proxy-path is mandatory, or dockerd exits (W-3.3 variant 1).
    put_file 0644 /etc/wsl.conf << 'EOF'
[boot]
command = "/usr/bin/env PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin /bin/sh -c 'exec /usr/bin/dockerd -H unix:///var/run/docker.sock --userland-proxy-path=/usr/bin/docker-proxy >/var/log/dockerd.log 2>&1'"

[user]
default = root

[interop]
appendWindowsPath = false
EOF
    # ENG-164: the base has no /etc/wsl-distribution.conf, so it is written fresh.
    put_file 0644 /etc/wsl-distribution.conf << 'EOF'
[shortcut]
enabled = false

[windowsterminal]
enabled = false
EOF
    grep -q '^docker:' "$ROOT/etc/group" || in_root /usr/sbin/addgroup -S docker
    for f in usr/bin/dockerd usr/bin/docker usr/bin/docker-proxy usr/bin/containerd usr/bin/runc usr/bin/socat usr/sbin/iptables; do
      [ -e "$ROOT/$f" ] || [ -L "$ROOT/$f" ] || die "the Alpine image lacks /$f"
    done
    for f in /usr/bin/dockerd /usr/bin/docker /usr/bin/containerd /usr/bin/runc; do check_elf_arch "$f"; done

    write_manifest_head
    {
      echo "packages_requested: ${APK_PACKAGES[*]}"
      echo "key_packages:"
      in_root /sbin/apk list --installed | awk '
        { split($0, w, " "); p = w[1]
          if (match(p, /-[0-9][^-]*-r[0-9]+$/)) { print substr(p, 1, RSTART - 1) " " substr(p, RSTART + 1) } }' \
        | grep -E '^(docker|docker-engine|docker-cli|docker-cli-buildx|containerd|runc|socat|iptables|ip6tables) ' | sort
      echo "all_packages:"
      in_root /sbin/apk list --installed | awk '
        { split($0, w, " "); p = w[1]
          if (match(p, /-[0-9][^-]*-r[0-9]+$/)) { print substr(p, 1, RSTART - 1) " " substr(p, RSTART + 1) } }' | sort
    } >> "$MANIFEST_TMP"
    in_root /bin/sh -s < "$HERE/guest/alpine-licenses.sh" > "$LICENSES_TMP"
    ;;

  ubuntu)
    DEB_ARCH=$(pin arch-get ubuntu "$ARCH" deb_arch)
    SUITE=$(pin get images.ubuntu.suite)
    APT_URL=$(pin get build.docker_apt_url)
    KEY_URL=$(pin get build.docker_apt_key_url)
    KEY_FPR=$(pin get build.docker_apt_key_fingerprint)
    mapfile -t APT_SPECS < <(pin apt-specs ubuntu)
    codename=$(sed -n 's/^VERSION_CODENAME=//p' "$ROOT/etc/os-release" | tr -d '"')
    [ "$codename" = "$SUITE" ] || die "base is '$codename' but pins.toml says suite '$SUITE'"

    # Docker's signing key: one key, and its primary fingerprint must equal the pinned one.
    curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --retry 3 \
      --connect-timeout 30 --max-filesize 65536 -o "$WORK/docker.asc" -- "$KEY_URL"
    [ "$(gpg --show-keys --with-colons "$WORK/docker.asc" | awk -F: '$1 == "pub"' | wc -l)" = 1 ] \
      || die "the Docker key file must hold exactly one key"
    got_fpr=$(gpg --show-keys --with-colons "$WORK/docker.asc" | awk -F: '$1 == "pub" { p = 1; next } p && $1 == "fpr" { print $10; exit }')
    [ "$got_fpr" = "$KEY_FPR" ] || die "Docker key fingerprint is '$got_fpr', pinned '$KEY_FPR'"
    "${SUDO[@]}" mkdir -p "$ROOT/etc/apt/keyrings" "$ROOT/etc/apt/sources.list.d"
    "${SUDO[@]}" install -m 0644 "$WORK/docker.asc" "$ROOT/etc/apt/keyrings/docker.asc"
    put_file 0644 /etc/apt/sources.list.d/docker.sources << EOF
Types: deb
URIs: $APT_URL
Suites: $SUITE
Components: stable
Architectures: $DEB_ARCH
Signed-By: /etc/apt/keyrings/docker.asc
EOF
    # Nothing may start inside the chroot (dpkg would try to start docker.service).
    put_file 0755 /usr/sbin/policy-rc.d << 'EOF'
#!/bin/sh
exit 101
EOF
    in_root /usr/bin/apt-get update
    if [ "$UPGRADE_BASE" = true ]; then
      in_root /usr/bin/apt-get -y -o Dpkg::Options::=--force-confold upgrade
    fi
    in_root /usr/bin/apt-get -y --no-install-recommends -o Dpkg::Options::=--force-confold install "${APT_SPECS[@]}"
    for spec in "${APT_SPECS[@]}"; do
      want=${spec#*=}
      # shellcheck disable=SC2016 # a dpkg-query format string, not a shell expansion
      got=$(in_root /usr/bin/dpkg-query -W -f='${Version}' "${spec%%=*}")
      [ "$got" = "$want" ] || die "${spec%%=*} is $got, pinned $want"
    done
    in_root /usr/bin/systemctl enable docker.service docker.socket containerd.service
    in_root /usr/bin/apt-get clean
    "${SUDO[@]}" rm -f -- "$ROOT/usr/sbin/policy-rc.d"

    # ENG-164: edit Ubuntu's shipped file in place. [oobe] (command, defaultUid, defaultName) and the
    # icon / terminal template stay as shipped; only the two switches are added. `wsl --import` never runs
    # the OOBE (spike W-3.1), so the default user is root, as wsl.conf says.
    [ -f "$ROOT/etc/wsl-distribution.conf" ] || die "the Ubuntu base has no /etc/wsl-distribution.conf"
    ini_set "$ROOT/etc/wsl-distribution.conf" shortcut enabled false
    ini_set "$ROOT/etc/wsl-distribution.conf" windowsterminal enabled false
    put_file 0644 /etc/wsl.conf << 'EOF'
[boot]
systemd=true

[user]
default=root

[interop]
appendWindowsPath=false
EOF
    grep -q '^docker:' "$ROOT/etc/group" || die "the docker group is missing"
    for f in usr/bin/dockerd usr/bin/docker usr/bin/docker-proxy usr/bin/containerd usr/bin/runc; do
      [ -e "$ROOT/$f" ] || [ -L "$ROOT/$f" ] || die "the Ubuntu image lacks /$f"
    done
    for f in /usr/bin/dockerd /usr/bin/docker /usr/bin/containerd /usr/bin/runc; do check_elf_arch "$f"; done
    for unit in multi-user.target.wants/docker.service multi-user.target.wants/containerd.service sockets.target.wants/docker.socket; do
      [ -L "$ROOT/etc/systemd/system/$unit" ] || die "$unit is not enabled"
    done

    write_manifest_head
    {
      echo "docker_apt_suite: $SUITE"
      echo "docker_apt_key_fingerprint: $KEY_FPR"
      echo "key_packages:"
      for spec in "${APT_SPECS[@]}"; do echo "${spec%%=*} ${spec#*=}"; done | sort
      echo "all_packages:"
      # shellcheck disable=SC2016 # a dpkg-query format string, not a shell expansion
      in_root /usr/bin/dpkg-query -W -f='${binary:Package} ${Version}\n' | sort
    } >> "$MANIFEST_TMP"
    in_root /bin/sh -s < "$HERE/guest/ubuntu-licenses.sh" > "$LICENSES_TMP"
    ;;
esac

# --- 3. common finish ----------------------------------------------------------------------------
"${SUDO[@]}" install -m 0644 "$MANIFEST_TMP" "$ROOT/etc/dockering-engine-manifest.txt"
restore_dns
restore_kept
umount_all

# Nothing that only the build needed may stay behind.
"${SUDO[@]}" find "$ROOT/var/cache/apk" "$ROOT/var/lib/apt/lists" "$ROOT/var/cache/apt" "$ROOT/tmp" "$ROOT/var/tmp" \
  -mindepth 1 -not -name lock -not -name partial -delete 2> /dev/null || true
"${SUDO[@]}" rm -rf -- "$ROOT/root/.cache" "$ROOT/root/.ash_history" "$ROOT/root/.bash_history" \
  "$ROOT/var/log/apk.log" "$ROOT/var/log/apt" "$ROOT/var/log/dpkg.log" "$ROOT/var/log/alternatives.log" \
  "$ROOT/var/log/bootstrap.log" "$ROOT/var/cache/ldconfig/aux-cache" "$ROOT/var/lib/dpkg/"*-old \
  "$ROOT/var/cache/debconf/"*-old
[ -z "$("${SUDO[@]}" ls -A "$ROOT/var/cache/apk" 2> /dev/null)" ] || die "/var/cache/apk is not empty"
[ -z "$("${SUDO[@]}" ls -A "$ROOT/var/lib/apt/lists" 2> /dev/null | grep -v '^partial$\|^lock$' || true)" ] || die "apt lists remain"

TAR=$WORK/$NAME.tar
note "creating the tar (sorted, mtime clamped to @$SOURCE_DATE_EPOCH, numeric owners, security.capability kept)"
"${SUDO[@]}" tar --create --file "$TAR" --directory "$ROOT" \
  --format=posix --pax-option='exthdr.name=%d/PaxHeaders/%f,delete=atime,delete=ctime' \
  --sort=name --mtime="@$SOURCE_DATE_EPOCH" --clamp-mtime --numeric-owner \
  --xattrs --xattrs-include='security.capability' --one-file-system .
TAR_SHA256=$("${SUDO[@]}" sha256sum -- "$TAR" | cut -d' ' -f1)

note "compressing (xz -9e, 4 threads, fixed block size)"
xz -9e -T4 --block-size=201326592 --stdout -- "$TAR" > "$OUT/$NAME.tar.xz"
xz --test -- "$OUT/$NAME.tar.xz"
[ "$(xz --decompress --stdout -- "$OUT/$NAME.tar.xz" | sha256sum | cut -d' ' -f1)" = "$TAR_SHA256" ] \
  || die "the compressed asset does not decompress to the tar that was built"
tar --list --file "$TAR" ./etc/wsl.conf ./etc/wsl-distribution.conf ./etc/dockering-engine-manifest.txt > /dev/null \
  || die "the tar lacks the image configuration files"

( cd -- "$OUT" && sha256sum -- "$NAME.tar.xz" > "$NAME.tar.xz.sha256" )
install -m 0644 "$MANIFEST_TMP" "$OUT/$NAME.manifest.txt"
install -m 0644 "$LICENSES_TMP" "$OUT/LICENSES-$IMAGE-$ARCH.txt"
python3 -B -I "$HERE/pins.py" entry "$IMAGE" "$ARCH" "$OUT/$NAME.tar.xz" "$OUT/$NAME.manifest.txt" "$TAG" "$TAR_SHA256" \
  > "$OUT/$NAME.json"
note "done: $(stat -c %s -- "$OUT/$NAME.tar.xz") bytes, sha256 $(cut -d' ' -f1 "$OUT/$NAME.tar.xz.sha256"), uncompressed tar sha256 $TAR_SHA256"
