#!/bin/bash
# Throwaway feasibility probe: can Dockering be installed, started, screenshotted and driven on a hosted macOS runner?
# Every section reports on its own: nothing here stops the run, and the script always exits 0.
set +e
OUT="${RUNNER_TEMP:-/tmp}/probe-out"
mkdir -p "$OUT/bin"
exec > >(tee -a "$OUT/probe.log") 2>&1

TAG="${PROBE_TAG:-v0.2.0}"
case "$(uname -m)" in arm64) A=aarch64 ;; *) A=x86_64 ;; esac
log() { printf '\n===== %s =====\n' "$*"; }
tmo() { local s=$1; shift; perl -e 'alarm shift; exec @ARGV or exit 127' "$s" "$@"; }

log "runner"
sw_vers
echo "arch: $(uname -m) -> package arch $A"
echo "cpu: $(sysctl -n machdep.cpu.brand_string 2>&1) | ncpu $(sysctl -n hw.ncpu) | mem $(( $(sysctl -n hw.memsize) / 1048576 )) MB"
echo "launchctl managername: $(launchctl managername 2>&1)"
echo "id: $(id)"
echo "console user: $(stat -f%Su /dev/console 2>&1)"
ps -axo pid,user,comm | grep -E 'WindowServer|loginwindow|/Dock$|/Finder$|hosted-compute|Runner.Worker|Runner.Listener' | grep -v grep | head -n 12
echo "bash: $(command -v bash) $(bash --version | head -n 1)"
for t in swift swiftc gtimeout timeout perl python3 gh osascript screencapture sips hdiutil codesign spctl lipo otool vtool plutil ditto; do printf '  %-14s %s\n' "$t" "$(command -v $t || echo MISSING)"; done

log "displays and GPU (system_profiler)"
tmo 90 system_profiler SPDisplaysDataType 2>&1 | head -n 40

log "swift helpers: Metal devices, displays, window list"
cd "$OUT/bin" || exit 0
cat > gpu.swift <<'EOF'
import Metal
import CoreGraphics
let all = MTLCopyAllDevices()
print("MTLCopyAllDevices: \(all.map { $0.name })")
if let d = MTLCreateSystemDefaultDevice() {
  print("MTLCreateSystemDefaultDevice: \(d.name) lowPower=\(d.isLowPower) removable=\(d.isRemovable) unifiedMemory=\(d.hasUnifiedMemory)")
} else {
  print("MTLCreateSystemDefaultDevice: NONE")
}
var ids = [CGDirectDisplayID](repeating: 0, count: 8)
var cnt: UInt32 = 0
CGGetActiveDisplayList(8, &ids, &cnt)
print("active displays: \(cnt), main id \(CGMainDisplayID())")
for i in 0..<Int(cnt) { print("  display \(ids[i]): \(CGDisplayPixelsWide(ids[i]))x\(CGDisplayPixelsHigh(ids[i]))") }
EOF
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
cat > ax.applescript <<'EOF'
on run argv
  set procName to item 1 of argv
  set maxRows to 300
  tell application "System Events"
    set ps to every process whose name contains procName
    if (count of ps) is 0 then return "NO PROCESS matching " & procName
    set p to item 1 of ps
    set out to "process: " & (name of p) & " | windows=" & (count of windows of p) & linefeed
    try
      repeat with w in windows of p
        set out to out & "window: " & (name of w) & " | " & (role of w) & linefeed
      end repeat
    end try
    try
      with timeout of 120 seconds
        set els to entire contents of window 1 of p
      end timeout
      set out to out & "elements: " & (count of els) & linefeed
      set i to 0
      repeat with e in els
        set i to i + 1
        if i > maxRows then exit repeat
        set r to ""
        set nm to ""
        set ds to ""
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
          set sel to (selected of e) as string
        end try
        set out to out & r & " | " & nm & " | " & ds & " | sel=" & sel & linefeed
      end repeat
    on error errMsg
      set out to out & "entire contents failed: " & errMsg & linefeed
    end try
  end tell
  return out
end run
EOF
time (tmo 240 swiftc -O gpu.swift -o gpu 2>&1 | tail -n 5)
time (tmo 240 swiftc -O winlist.swift -o winlist 2>&1 | tail -n 5)
tmo 30 ./gpu

log "screen capture of the empty desktop"
tmo 30 screencapture -x "$OUT/00-desktop.png"
echo "screencapture rc=$?"
sips -g pixelWidth -g pixelHeight "$OUT/00-desktop.png" 2>&1 | tail -n 3
ls -l "$OUT/00-desktop.png"

log "download and verify the published DMG ($TAG, $A)"
mkdir -p "$OUT/dl" && cd "$OUT/dl" || exit 0
gh release download "$TAG" --repo "$GITHUB_REPOSITORY" --pattern "Dockering-$A.dmg" --pattern SHA256SUMS --dir . 2>&1 | tail -n 3
ls -l
grep -E "^[0-9a-f]{64} [ *]Dockering-$A\.dmg\$" SHA256SUMS > one.sha256
cat one.sha256
shasum -a 256 -c one.sha256
echo "checksum rc=$?"
gh attestation verify "Dockering-$A.dmg" --repo "$GITHUB_REPOSITORY" --signer-workflow "$GITHUB_REPOSITORY/.github/workflows/release.yml" --source-ref "refs/tags/$TAG" --deny-self-hosted-runners 2>&1 | tail -n 4
echo "attestation rc=${PIPESTATUS[0]}"

log "install from the DMG into /Applications"
MNT="$OUT/mnt"
mkdir -p "$MNT"
tmo 120 hdiutil attach "$OUT/dl/Dockering-$A.dmg" -nobrowse -readonly -noverify -mountpoint "$MNT" 2>&1 | tail -n 4
echo "attach rc=${PIPESTATUS[0]}"
ls -la "$MNT"
APP_SRC=$(ls -d "$MNT"/*.app 2>/dev/null | head -n 1)
echo "app in the dmg: $APP_SRC"
rm -rf /Applications/Dockering.app
ditto "$APP_SRC" /Applications/Dockering.app
echo "ditto rc=$?"
hdiutil detach "$MNT" -force 2>&1 | tail -n 2
APP=/Applications/Dockering.app
find "$APP" -maxdepth 3 | head -n 40
du -sh "$APP"

log "bundle inspection"
plutil -p "$APP/Contents/Info.plist" | head -n 40
EXE=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$APP/Contents/Info.plist" 2>/dev/null)
BIN="$APP/Contents/MacOS/$EXE"
echo "executable: $BIN"
file "$BIN"
lipo -info "$BIN"
vtool -show-build "$BIN" 2>&1 | head -n 10
otool -L "$BIN" | head -n 40
echo "--- codesign"
codesign -dv --verbose=4 "$APP" 2>&1 | head -n 16
codesign --verify --deep --strict --verbose=2 "$APP" 2>&1 | head -n 6
echo "--- gatekeeper (spctl)"
spctl --assess --type execute --verbose=4 "$APP" 2>&1 | head -n 6
echo "--- xattrs"
xattr -lr "$APP" 2>&1 | head -n 6
echo "--- version"
"$BIN" --version
echo "version rc=$?"

run_ax() { tmo 200 osascript "$OUT/bin/ax.applescript" ockering 2>&1; }

log "launch 1: the bundle executable run directly, --demo"
rm -rf "$HOME/Library/Application Support/dev.dockering.Dockering" "${TMPDIR:-/tmp}"/dockering-demo-* 2>/dev/null
RUST_LOG=info "$BIN" --demo >"$OUT/app1.out" 2>"$OUT/app1.err" &
APID=$!
echo "pid $APID"
W=""
for i in $(seq 1 80); do
  W=$("$OUT/bin/winlist" ockering 2>/dev/null | head -n 1)
  [ -n "$W" ] && break
  kill -0 $APID 2>/dev/null || { echo "the app exited early"; break; }
  sleep 0.5
done
echo "window found after about $((i / 2)) s: $W"
echo "--- all windows of the process:"
"$OUT/bin/winlist" ockering
if kill -0 $APID 2>/dev/null; then echo "app alive"; else wait $APID; echo "app exited, status $?"; fi
sleep 4
tmo 30 screencapture -x "$OUT/01-full.png"
echo "full-screen capture rc=$?"
WID=$(printf '%s' "$W" | cut -f1)
if [ -n "$WID" ]; then
  tmo 30 screencapture -x -o -l "$WID" "$OUT/01-window.png"
  echo "window capture rc=$?"
fi
for f in "$OUT"/01-*.png; do [ -f "$f" ] && echo "$(basename "$f"): $(sips -g pixelWidth -g pixelHeight "$f" 2>&1 | tr '\n' ' ' | sed 's/  */ /g') $(stat -f%z "$f") bytes"; done
echo "--- app stderr (first 25 lines)"
head -n 25 "$OUT/app1.err"
echo "--- log files"
find "$HOME/Library" "${TMPDIR:-/tmp}" -maxdepth 5 -name 'dockering*.log*' 2>/dev/null | head -n 5

log "accessibility tree through System Events (before any key)"
run_ax | tee "$OUT/ax-1.txt" | head -n 70
echo "ax rc=${PIPESTATUS[0]}"

log "keyboard through System Events: bring to front, Cmd+2, Cmd+3, Cmd+4, Cmd+1"
osascript -e 'tell application "System Events" to set frontmost of (first process whose name contains "ockering") to true' 2>&1
echo "frontmost rc=$?"
sleep 1
for k in 2 3 4 1; do
  osascript -e "tell application \"System Events\" to keystroke \"$k\" using {command down}" 2>&1
  echo "keystroke Cmd+$k rc=$?"
  sleep 2
  tmo 30 screencapture -x "$OUT/02-after-cmd-$k.png"
done
log "accessibility tree after the keys (the sidebar selection should be Containers again)"
run_ax | tee "$OUT/ax-2.txt" | grep -i -E 'process:|window:|elements:|row|outline|tab|sel=true' | head -n 40

log "quit with Cmd+Q"
osascript -e 'tell application "System Events" to keystroke "q" using {command down}' 2>&1
echo "keystroke rc=$?"
for i in $(seq 1 40); do kill -0 $APID 2>/dev/null || break; sleep 0.25; done
if kill -0 $APID 2>/dev/null; then echo "STILL RUNNING 10 s after Cmd+Q"; kill $APID; else wait $APID; echo "app exit status: $?"; fi
echo "--- state files"
find "${TMPDIR:-/tmp}" "$HOME/Library/Application Support" -maxdepth 5 \( -name 'state.json' -o -name 'crash-*.txt' \) 2>/dev/null | head -n 6
cat "${TMPDIR:-/tmp}"/dockering-demo-*/data/state.json 2>/dev/null | head -c 300; echo

log "launch 2: open -n -a (LaunchServices, like a double click), --demo"
open -n -a "$APP" --args --demo
echo "open rc=$?"
W=""
for i in $(seq 1 80); do
  W=$("$OUT/bin/winlist" ockering 2>/dev/null | head -n 1)
  [ -n "$W" ] && break
  sleep 0.5
done
echo "window found after about $((i / 2)) s: $W"
sleep 4
tmo 30 screencapture -x "$OUT/10-open-full.png"
lsappinfo list 2>&1 | grep -i -B1 -A5 dockering | head -n 14
pgrep -fl 'Dockering.app/Contents/MacOS' | head -n 3
osascript -e 'tell application "System Events" to set frontmost of (first process whose name contains "ockering") to true' 2>&1
sleep 1
osascript -e 'tell application "System Events" to keystroke "q" using {command down}' 2>&1
for i in $(seq 1 40); do pgrep -f 'Dockering.app/Contents/MacOS' >/dev/null || break; sleep 0.25; done
if pgrep -f 'Dockering.app/Contents/MacOS' >/dev/null; then echo "still running after Cmd+Q"; pkill -f 'Dockering.app/Contents/MacOS'; else echo "app quit"; fi

log "launch 3: a copy marked as downloaded by a browser (quarantine), the way a user gets it"
rm -rf /tmp/q && mkdir -p /tmp/q
ditto "$APP" /tmp/q/Dockering.app
xattr -w com.apple.quarantine "0081;$(printf '%x' "$(date +%s)");Safari;" /tmp/q/Dockering.app
xattr -l /tmp/q/Dockering.app
spctl --assess --type execute --verbose=4 /tmp/q/Dockering.app 2>&1 | head -n 4
open -n -a /tmp/q/Dockering.app --args --demo
echo "open rc=$?"
sleep 10
echo "--- windows of the app:"
"$OUT/bin/winlist" ockering
echo "--- windows of any Gatekeeper helper:"
"$OUT/bin/winlist" CoreServicesUIAgent
"$OUT/bin/winlist" UserNotificationCenter
tmo 30 screencapture -x "$OUT/20-quarantine-full.png"
pgrep -fl 'q/Dockering.app' | head -n 3
pkill -f '/tmp/q/Dockering.app' 2>/dev/null

log "done"
sleep 2
exit 0
