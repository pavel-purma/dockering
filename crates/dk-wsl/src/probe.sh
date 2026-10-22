d=/var/run/docker.sock
if test -S $d; then
  if command -v dockerd >/dev/null 2>&1; then
    run=
    for c in /proc/[0-9]*/comm; do { read -r n < "$c"; } 2>/dev/null && test "$n" = dockerd && run=1 && break; done
    if test -z "$run"; then
      if test -d /run/systemd/system && systemctl is-active --quiet docker.socket 2>/dev/null; then :; else exit 14; fi
    fi
  fi
  t=$(command -v docker || command -v socat) || exit 12
  echo "$t"
  test -r $d && test -w $d || exit 13
else
  if command -v dockerd >/dev/null 2>&1 || test -e /etc/init.d/docker || test -e /usr/lib/systemd/system/docker.service || test -e /lib/systemd/system/docker.service; then exit 14; fi
  exit 11
fi
