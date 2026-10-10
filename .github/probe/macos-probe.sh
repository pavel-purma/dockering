#!/bin/bash
# Throwaway probe, round 2: the questions round 1 left open. Every step has its own hard timeout; the script always exits 0.
set +e
OUT="${RUNNER_TEMP:-/tmp}/probe-out"
mkdir -p "$OUT/bin"
exec > >(tee -a "$OUT/probe.log") 2>&1
TAG="${PROBE_TAG:-v0.2.0}"
case "$(uname -m)" in arm64) A=aarch64 ;; *) A=x86_64 ;; esac
log() { printf '\n===== %s =====\n' "$*"; }
# run a command with a hard time limit; prints "[timeout after N s]" and returns 124 when it is killed
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
winlist() { "$OUT/bin/winlist" "$@" 2>/dev/null; }
shot() { tmo 30 screencapture -x "$OUT/$1.png"; echo "screencapture $1 rc=$?"; }

log "runner"
sw_vers | tr '\n' ' '; echo
echo "arch $(uname -m)  bash $BASH_VERSION  user $(id -un)"

log "helpers: window list (CoreGraphics), compiled once"
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
  let name = (w[kCGWindowName as String] as? String) ?? ""
  let layer = (w[kCGWindowLayer as String] as? Int) ?? -1
  let on = (w[kCGWindowIsOnscreen as String] as? Bool) ?? false
  let b = (w[kCGWindowBounds as String] as? [String: Any]) ?? [:]
  print("\(num)\t\(owner)\t\(name)\tlayer=\(layer)\tonscreen=\(on)\tbounds=\(b["X"] ?? "?"),\(b["Y"] ?? "?"),\(b["Width"] ?? "?"),\(b["Height"] ?? "?")")
  n += 1
}
exit(n > 0 ? 0 : 1)
EOF
tmo 240 swiftc -O winlist.swift -o winlist 2>&1 | tail -n 3
ls -l winlist

log "download, verify and install the published DMG ($TAG, $A)"
mkdir -p "$OUT/dl" && cd "$OUT/dl" || exit 0
tmo 120 gh release download "$TAG" --repo "$GITHUB_REPOSITORY" --pattern "Dockering-$A.dmg" --pattern SHA256SUMS --dir . 2>&1 | tail -n 2
grep -E "^[0-9a-f]{64} [ *]Dockering-$A\.dmg\$" SHA256SUMS > one.sha256
shasum -a 256 -c one.sha256
MNT="$OUT/mnt"
mkdir -p "$MNT"
tmo 120 hdiutil attach "$OUT/dl/Dockering-$A.dmg" -nobrowse -readonly -noverify -mountpoint "$MNT" 2>&1 | tail -n 2
rm -rf "$APP"
ditto "$MNT/Dockering.app" "$APP"
tmo 60 hdiutil detach "$MNT" -force 2>&1 | tail -n 1
echo "installed: $(ls "$APP/Contents/MacOS")"

log "signature state of the bundle as published (three views)"
echo "--- codesign -dv"; codesign -dv --verbose=2 "$APP" 2>&1 | head -n 8
echo "--- codesign --verify --deep --strict"; codesign --verify --deep --strict "$APP" 2>&1 | head -n 3; echo "rc=$?"
echo "--- spctl"; spctl --assess --type execute --verbose=4 "$APP" 2>&1 | head -n 3
echo "--- the binary alone"; codesign -dv "$BIN" 2>&1 | head -n 4; codesign --verify --strict "$BIN" 2>&1 | head -n 2

log "A: launch the bundle through LaunchServices with no quarantine flag (open -n -a), --demo"
rm -rf "${TMPDIR:-/tmp}"/dockering-demo-* 2>/dev/null
tmo 30 open -n -a "$APP" --args --demo
echo "open rc=$?"
for i in $(seq 1 40); do winlist ockering | head -n 1 | grep -q . && break; sleep 0.5; done
echo "window after about $((i / 2)) s:"; winlist ockering
sleep 3
shot A-open-plain
pgrep -fl "Dockering.app/Contents/MacOS" | head -n 2
echo "--- quit with Cmd+Q through System Events (hard limit 20 s)"
tmo 20 osascript -e 'tell application "System Events" to set frontmost of (first process whose name contains "ockering") to true'
sleep 1
tmo 20 osascript -e 'tell application "System Events" to keystroke "q" using {command down}'
echo "keystroke rc=$?"
for i in $(seq 1 40); do pgrep -f "Dockering.app/Contents/MacOS" >/dev/null || break; sleep 0.25; done
pgrep -f "Dockering.app/Contents/MacOS" >/dev/null && { echo "STILL RUNNING after Cmd+Q"; pkill -9 -f "Dockering.app/Contents/MacOS"; } || echo "app quit"

log "B: a copy that carries the browser download flag (quarantine), the way a user gets it"
rm -rf /tmp/q && mkdir -p /tmp/q
ditto "$APP" /tmp/q/Dockering.app
xattr -w com.apple.quarantine "0081;$(printf '%x' "$(date +%s)");Safari;" /tmp/q/Dockering.app
xattr -l /tmp/q/Dockering.app
echo "--- spctl on the quarantined copy (hard limit 30 s)"
tmo 30 spctl --assess --type execute --verbose=4 /tmp/q/Dockering.app 2>&1 | head -n 4
echo "--- open it (hard limit 30 s); open itself returns at once"
tmo 30 open -n -a /tmp/q/Dockering.app --args --demo
echo "open rc=$?"
for i in $(seq 1 24); do winlist ockering | head -n 1 | grep -q . && break; sleep 0.5; done
echo "app window after about $((i / 2)) s:"; winlist ockering
sleep 6
echo "--- processes now"
pgrep -fl "q/Dockering.app" | head -n 3
ps -axo pid,comm | grep -i -E 'CoreServicesUIAgent|UserNotification|syspolicyd|XprotectService' | grep -v grep | head -n 6
echo "--- windows of the Gatekeeper helpers"
winlist CoreServicesUIAgent
winlist UserNotificationCenter
winlist SecurityAgent
shot B-open-quarantined
echo "--- clean up B (hard kill: the dialog, if any, may hold the app)"
pkill -9 -f "q/Dockering.app" 2>/dev/null
pkill -9 -f CoreServicesUIAgent 2>/dev/null
sleep 1

log "C: the same quarantined copy started as a plain process (what 'open' does not do)"
rm -rf "${TMPDIR:-/tmp}"/dockering-demo-* 2>/dev/null
( RUST_LOG=info "/tmp/q/Dockering.app/Contents/MacOS/dockering" --demo >"$OUT/C.out" 2>"$OUT/C.err" & echo $! >"$OUT/C.pid" )
sleep 8
CP=$(cat "$OUT/C.pid")
kill -0 "$CP" 2>/dev/null && echo "process alive, window:" || echo "process exited"
winlist ockering
head -n 6 "$OUT/C.err"
kill -9 "$CP" 2>/dev/null

log "D: ad-hoc re-signing (what a release step could do) then Gatekeeper's view of the quarantined copy"
rm -rf /tmp/r && mkdir -p /tmp/r
ditto "$APP" /tmp/r/Dockering.app
codesign --force --deep --sign - /tmp/r/Dockering.app 2>&1 | head -n 3
echo "--- verify: "; codesign --verify --deep --strict --verbose=2 /tmp/r/Dockering.app 2>&1 | head -n 4
xattr -w com.apple.quarantine "0081;$(printf '%x' "$(date +%s)");Safari;" /tmp/r/Dockering.app
tmo 30 spctl --assess --type execute --verbose=4 /tmp/r/Dockering.app 2>&1 | head -n 4
tmo 30 open -n -a /tmp/r/Dockering.app --args --demo
for i in $(seq 1 24); do winlist ockering | head -n 1 | grep -q . && break; sleep 0.5; done
echo "resigned+quarantined window after about $((i / 2)) s:"; winlist ockering
sleep 4
shot D-resigned-quarantined
pkill -9 -f "r/Dockering.app" 2>/dev/null
pkill -9 -f CoreServicesUIAgent 2>/dev/null

log "E: real mode (no --demo), first launch with a clean profile: window, log, state"
rm -rf "$HOME/Library/Application Support/dev.dockering.Dockering" "$HOME/Library/Logs/dev.dockering.Dockering" 2>/dev/null
tmo 30 open -n -a "$APP"
for i in $(seq 1 40); do winlist ockering | head -n 1 | grep -q . && break; sleep 0.5; done
echo "window after about $((i / 2)) s:"; winlist ockering
sleep 5
shot E-real-first-run
echo "--- where it keeps files"
find "$HOME/Library" -maxdepth 4 -path '*ockering*' 2>/dev/null | head -n 12
find "$HOME/Library" -maxdepth 5 -name 'dockering*.log*' 2>/dev/null | head -n 3
LOGF=$(find "$HOME/Library" -maxdepth 5 -name 'dockering*.log*' 2>/dev/null | head -n 1)
[ -n "$LOGF" ] && head -n 8 "$LOGF" | cut -c1-200
tmo 20 osascript -e 'tell application "System Events" to set frontmost of (first process whose name contains "ockering") to true'
sleep 1
tmo 20 osascript -e 'tell application "System Events" to keystroke "q" using {command down}'
for i in $(seq 1 40); do pgrep -f "Dockering.app/Contents/MacOS" >/dev/null || break; sleep 0.25; done
pgrep -f "Dockering.app/Contents/MacOS" >/dev/null && { echo "STILL RUNNING after Cmd+Q"; pkill -9 -f "Dockering.app/Contents/MacOS"; } || echo "app quit"
find "$HOME/Library" -maxdepth 5 \( -name 'state.json' -o -name 'crash-*.txt' \) 2>/dev/null | head -n 4

log "F: the accessibility read, with a hard limit, and how long it takes"
rm -rf "${TMPDIR:-/tmp}"/dockering-demo-* 2>/dev/null
tmo 30 open -n -a "$APP" --args --demo
for i in $(seq 1 40); do winlist ockering | head -n 1 | grep -q . && break; sleep 0.5; done
cat > "$OUT/bin/ax.applescript" <<'EOF'
on run argv
  set procName to item 1 of argv
  tell application "System Events"
    set ps to every process whose name contains procName
    if (count of ps) is 0 then return "NO PROCESS"
    set p to item 1 of ps
    set out to "windows=" & (count of windows of p) & linefeed
    try
      set rowsList to every UI element of window 1 of p whose role is "AXRow"
    on error
      set rowsList to {}
    end try
    set out to out & "direct rows of the window: " & (count of rowsList) & linefeed
    try
      set els to entire contents of window 1 of p
      set out to out & "elements: " & (count of els) & linefeed
      repeat with e in els
        set r to ""
        set nm to ""
        set sel to ""
        try
          set r to role of e
        end try
        try
          set nm to name of e
        end try
        try
          set sel to (value of attribute "AXSelected" of e) as string
        end try
        if r is "AXRow" and nm is not "missing value" and nm is not "" then set out to out & "row | " & nm & " | selected=" & sel & linefeed
      end repeat
    on error errMsg
      set out to out & "entire contents failed: " & errMsg & linefeed
    end try
  end tell
  return out
end run
EOF
for n in 1 2 3; do
  s=$(date +%s)
  tmo 90 osascript "$OUT/bin/ax.applescript" ockering 2>&1 | head -n 12
  echo "ax read $n took $(( $(date +%s) - s )) s"
  sleep 2
done
tmo 20 osascript -e 'tell application "System Events" to set frontmost of (first process whose name contains "ockering") to true'
sleep 1
for k in 2 4 1; do
  tmo 20 osascript -e "tell application \"System Events\" to keystroke \"$k\" using {command down}"
  sleep 2
  echo "after Cmd+$k:"
  tmo 90 osascript "$OUT/bin/ax.applescript" ockering 2>&1 | grep -E '^row \| (Containers|Images|Volumes|Networks)'
done
tmo 20 osascript -e 'tell application "System Events" to keystroke "q" using {command down}'
sleep 3
pkill -9 -f "Dockering.app/Contents/MacOS" 2>/dev/null

log "done"
exit 0
