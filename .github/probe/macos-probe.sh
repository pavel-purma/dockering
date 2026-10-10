#!/bin/bash
# Throwaway probe, round 3: the facts the macOS smoke harness depends on. Hard time limit on every step; always exits 0.
set +e
OUT="${RUNNER_TEMP:-/tmp}/probe-out"
mkdir -p "$OUT/bin"
exec > >(tee -a "$OUT/probe.log") 2>&1
TAG="${PROBE_TAG:-v0.2.0}"
case "$(uname -m)" in arm64) A=aarch64 ;; *) A=x86_64 ;; esac
log() { printf '\n===== %s =====\n' "$*"; }
tmo() {
  local s=$1 pid watcher rc
  shift
  "$@" &
  pid=$!
  ( sleep "$s"; kill -9 "$pid" 2>/dev/null && echo "[timeout after ${s}s: $*]" ) &
  watcher=$!
  wait "$pid" 2>/dev/null
  rc=$?
  kill "$watcher" 2>/dev/null
  wait "$watcher" 2>/dev/null
  return $rc
}
APP=/Applications/Dockering.app
BIN="$APP/Contents/MacOS/dockering"
DATA="$HOME/Library/Application Support/dev.dockering.Dockering"
winlist() { "$OUT/bin/winlist" "$@" 2>/dev/null; }
shot() { tmo 30 screencapture -x "$OUT/$1.png"; }

log "runner"
sw_vers | tr '\n' ' '; echo
cd "$OUT/bin" || exit 0
cat > winlist.swift <<'EOF'
import CoreGraphics
import Foundation
let needle = (CommandLine.arguments.dropFirst().first ?? "dockering").lowercased()
let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] ?? []
var n = 0
for w in list {
  let owner = (w[kCGWindowOwnerName as String] as? String) ?? ""
  guard owner.lowercased().contains(needle) else { continue }
  let num = (w[kCGWindowNumber as String] as? Int) ?? -1
  let on = (w[kCGWindowIsOnscreen as String] as? Bool) ?? false
  let layer = (w[kCGWindowLayer as String] as? Int) ?? -1
  let b = (w[kCGWindowBounds as String] as? [String: Any]) ?? [:]
  print("\(num)\t\(owner)\tlayer=\(layer)\tonscreen=\(on)\tbounds=\(b["X"] ?? "?"),\(b["Y"] ?? "?"),\(b["Width"] ?? "?"),\(b["Height"] ?? "?")")
  n += 1
}
exit(n > 0 ? 0 : 1)
EOF
tmo 240 swiftc -O winlist.swift -o winlist 2>&1 | tail -n 2
cat > ax.applescript <<'EOF'
on run argv
  set thePid to (item 1 of argv) as integer
  tell application "System Events"
    set ps to every process whose unix id is thePid
    if (count of ps) is 0 then return "NO PROCESS with pid " & thePid
    set p to item 1 of ps
    set out to "process: " & (name of p) & " windows=" & (count of windows of p) & linefeed
    try
      set els to entire contents of window 1 of p
    on error errMsg
      return out & "entire contents failed: " & errMsg
    end try
    set out to out & "elements: " & (count of els) & linefeed
    repeat with e in els
      set r to ""
      set nm to ""
      set ds to ""
      set tt to ""
      set hp to ""
      set sel to ""
      try
        set r to role of e
      end try
      try
        set nm to name of e
      end try
      try
        set ds to description of e
      end try
      try
        set tt to (value of attribute "AXTitle" of e) as string
      end try
      try
        set hp to (value of attribute "AXHelp" of e) as string
      end try
      try
        set sel to (value of attribute "AXSelected" of e) as string
      end try
      if r is not "" then set out to out & r & tab & nm & tab & ds & tab & tt & tab & hp & tab & sel & linefeed
    end repeat
    return out
  end tell
end run
EOF
cat > rowpos.applescript <<'EOF'
on run argv
  set thePid to (item 1 of argv) as integer
  set wanted to item 2 of argv
  tell application "System Events"
    set p to first process whose unix id is thePid
    set els to entire contents of window 1 of p
    repeat with e in els
      try
        if (role of e) is "AXRow" and (name of e) is wanted then
          set pp to position of e
          set ss to size of e
          return ((item 1 of pp) as string) & " " & ((item 2 of pp) as string) & " " & ((item 1 of ss) as string) & " " & ((item 2 of ss) as string)
        end if
      end try
    end repeat
    return "NOT FOUND"
  end tell
end run
EOF
ax() { tmo 170 osascript "$OUT/bin/ax.applescript" "$1" 2>&1; }
front() { tmo 20 osascript -e "tell application \"System Events\" to set frontmost of (first process whose unix id is $1) to true" 2>&1; sleep 1; }
cmd() { tmo 20 osascript -e "tell application \"System Events\" to keystroke \"$2\" using {command down}" 2>&1; }

log "install the published DMG ($TAG, $A)"
mkdir -p "$OUT/dl" && cd "$OUT/dl" || exit 0
tmo 120 gh release download "$TAG" --repo "$GITHUB_REPOSITORY" --pattern "Dockering-$A.dmg" --dir . 2>&1 | tail -n 1
mkdir -p "$OUT/mnt"
tmo 120 hdiutil attach "$OUT/dl/Dockering-$A.dmg" -nobrowse -readonly -noverify -mountpoint "$OUT/mnt" 2>&1 | tail -n 1
rm -rf "$APP"; ditto "$OUT/mnt/Dockering.app" "$APP"; hdiutil detach "$OUT/mnt" -force >/dev/null 2>&1
echo "binary: $("$BIN" --version)"

waitwin() { local i; for i in $(seq 1 60); do winlist ockering | grep -q 'layer=0.*onscreen=true' && return 0; sleep 0.5; done; return 1; }
killall_app() { pkill -9 -f "Dockering.app/Contents/MacOS" 2>/dev/null; sleep 1; }

log "U: the upgrade profile (real mode, last_run_version 0.0.1): what does the accessibility tree show, and for how long"
killall_app
rm -rf "$DATA" "$HOME/Library/Preferences/dev.dockering.Dockering.plist"
mkdir -p "$DATA"
printf '{"updates":{"last_run_version":"0.0.1"}}\n' >"$DATA/state.json"
RUST_LOG=info "$BIN" >"$OUT/U.out" 2>"$OUT/U.err" &
UP=$!
waitwin && echo "window up"
for n in 1 2 3; do
  s=$(date +%s)
  ax $UP >"$OUT/U-ax-$n.txt"
  echo "U ax $n: $(( $(date +%s) - s )) s, $(wc -l <"$OUT/U-ax-$n.txt") lines"
  shot "U-$n"
  sleep 3
done
echo "--- buttons with any name/title/description/help in the upgrade scenario (first dump):"
awk -F'\t' '$1=="AXButton" && ($2!="missing value" || $3!="button" || $4!="missing value" || $5!="missing value") {print}' "$OUT/U-ax-1.txt" | head -n 12
echo "--- anything mentioning What/update/0.2.0:"
grep -a -i -E "what|updat|0\.2\.0" "$OUT/U-ax-1.txt" "$OUT/U-ax-2.txt" "$OUT/U-ax-3.txt" | head -n 8
echo "--- the log after the upgrade launch:"
tail -n 5 "$DATA/logs/"dockering.*.log 2>/dev/null | cut -c1-200
echo "--- Settings with Cmd+,"
front $UP
cmd $UP ","
sleep 3
ax $UP >"$OUT/S-ax.txt"
awk -F'\t' '$1=="AXRow" {print $2 "\t" $6}' "$OUT/S-ax.txt" | head -n 14
shot S-settings
cmd $UP "1"
sleep 2

log "K: click a sidebar row with a real pointer event (System Events click at)"
front $UP
tmo 170 osascript "$OUT/bin/rowpos.applescript" $UP Images
POS=$(tmo 170 osascript "$OUT/bin/rowpos.applescript" $UP Images 2>&1 | tail -n 1)
echo "row Images at: $POS"
set -- $POS
if [ "$1" != "NOT" ] && [ -n "$1" ]; then
  CX=$(( ${1%.*} + ${3%.*} / 2 )); CY=$(( ${2%.*} + ${4%.*} / 2 ))
  echo "clicking at $CX,$CY"
  tmo 20 osascript -e "tell application \"System Events\" to click at {$CX, $CY}" 2>&1
  sleep 2
  ax $UP >"$OUT/K-ax.txt"
  awk -F'\t' '$1=="AXRow" && ($2=="Containers"||$2=="Images"||$2=="Volumes"||$2=="Networks") {print $2 "\t" $6}' "$OUT/K-ax.txt"
  shot K-after-click
fi

log "Q: quit variants on the REAL-mode process (it did not quit with Cmd+Q in round 2)"
echo "pid $UP alive: $(kill -0 $UP 2>/dev/null && echo yes || echo no)"
front $UP
echo "frontmost process: $(tmo 20 osascript -e 'tell application "System Events" to name of first process whose frontmost is true' 2>&1)"
cmd $UP "q"
for i in $(seq 1 40); do kill -0 $UP 2>/dev/null || break; sleep 0.25; done
if kill -0 $UP 2>/dev/null; then
  echo "STILL RUNNING 10 s after Cmd+Q (pid-targeted frontmost)"
  echo "--- try: tell application Dockering to quit"
  tmo 20 osascript -e 'tell application "Dockering" to quit' 2>&1
  for i in $(seq 1 40); do kill -0 $UP 2>/dev/null || break; sleep 0.25; done
  kill -0 $UP 2>/dev/null && echo "STILL RUNNING after osascript quit" || echo "quit by osascript"
fi
if kill -0 $UP 2>/dev/null; then
  echo "--- try: Cmd+W"
  front $UP
  cmd $UP "w"
  for i in $(seq 1 40); do kill -0 $UP 2>/dev/null || break; sleep 0.25; done
  kill -0 $UP 2>/dev/null && echo "STILL RUNNING after Cmd+W" || echo "quit by Cmd+W"
fi
if kill -0 $UP 2>/dev/null; then
  echo "--- try: SIGTERM"
  kill -TERM $UP
  for i in $(seq 1 40); do kill -0 $UP 2>/dev/null || break; sleep 0.25; done
  kill -0 $UP 2>/dev/null && echo "STILL RUNNING after SIGTERM" || echo "quit by SIGTERM"
fi
wait $UP 2>/dev/null
echo "real-mode exit status: $?"
killall_app
echo "--- state.json now:"; cat "$DATA/state.json" 2>/dev/null; echo
echo "--- distinct WARN/ERROR lines in the real-mode log:"
grep -a -h -E ' (WARN|ERROR) ' "$DATA/logs/"dockering.*.log 2>/dev/null | sed -E 's/^[0-9T:.Z-]+ +//' | cut -c1-170 | sort | uniq -c

log "D: demo mode started directly (exit status of a clean Cmd+Q), then through LaunchServices"
rm -rf "${TMPDIR:-/tmp}"/dockering-demo-* 2>/dev/null
RUST_LOG=info "$BIN" --demo >"$OUT/D.out" 2>"$OUT/D.err" &
DP=$!
waitwin && echo "window up"
front $DP
cmd $DP "q"
for i in $(seq 1 40); do kill -0 $DP 2>/dev/null || break; sleep 0.25; done
kill -0 $DP 2>/dev/null && echo "STILL RUNNING after Cmd+Q" || echo "quit"
wait $DP 2>/dev/null
echo "demo exit status: $?"
echo "--- demo state and log paths:"
find "${TMPDIR:-/tmp}" -maxdepth 4 -path '*dockering-demo-*' \( -name 'state.json' -o -name '*.log' -o -name 'crash-*' \) 2>/dev/null | head -n 5
cat "${TMPDIR:-/tmp}"/dockering-demo-*/data/state.json 2>/dev/null; echo
echo "--- distinct WARN/ERROR lines in the demo log:"
grep -a -h -E ' (WARN|ERROR) ' "${TMPDIR:-/tmp}"/dockering-demo-*/logs/dockering.*.log 2>/dev/null | sed -E 's/^[0-9T:.Z-]+ +//' | cut -c1-170 | sort | uniq -c
mkdir -p "$OUT/logs"
cp "$DATA/logs/"dockering.*.log "$OUT/logs/real.log" 2>/dev/null
cp "${TMPDIR:-/tmp}"/dockering-demo-*/logs/dockering.*.log "$OUT/logs/demo.log" 2>/dev/null

log "done"
exit 0
