#!/bin/bash
# Regression check for the sidebar drift bug: crossing a layout breakpoint used
# to move the selection down one row (Home → Pinned → Archived), because GTK
# restores focus by advancing the cursor and SelectionMode::Single selects as it
# goes. Clicks "Pinned", resizes across both breakpoints and back, and asserts
# the sidebar looks identical afterwards.
#
# Offscreen only — never touches the real desktop. Needs the debug build.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DISP=":99"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
export XDG_CONFIG_HOME="$WORK/cfg" XDG_DATA_HOME="$WORK/data"
mkdir -p "$XDG_DATA_HOME/Notes/notes"
now=$(date +%s000)
cat > "$XDG_DATA_HOME/Notes/notes/a.json" <<EOF
{"id":"a","title":"Alpha","content":"one","type":"TEXT","pinned":false,
 "archived":false,"trashed":false,"createdAt":$now,"updatedAt":$now,
 "syncStatus":"LOCAL_ONLY","labels":["work"]}
EOF

pkill -f "Xvfb $DISP" 2>/dev/null; sleep 0.3
Xvfb "$DISP" -screen 0 1400x900x24 >/dev/null 2>&1 & XVFB=$!
sleep 1
DISPLAY="$DISP" GDK_BACKEND=x11 DBUS_SESSION_BUS_ADDRESS=disabled: \
  "$ROOT/target/debug/notes" > "$WORK/app.log" 2>&1 & APP=$!
sleep 6
W=$(DISPLAY=$DISP xdotool search --onlyvisible --name "." 2>/dev/null | tail -1)
[ -n "$W" ] || { echo "FAIL: no window (see $WORK/app.log)"; kill $APP $XVFB; exit 1; }

# Park the pointer off the sidebar first: a hovered row renders differently and
# would fail the comparison for the wrong reason.
shot() { DISPLAY=$DISP xdotool mousemove 700 700; sleep 1; python3 -c "
from PIL import ImageGrab
ImageGrab.grab(xdisplay='$DISP').crop((0,60,225,260)).save('$WORK/$1.png')"; }

DISPLAY=$DISP xdotool windowsize $W 1350 850;      sleep 2
DISPLAY=$DISP xdotool mousemove 100 140 click 1;   sleep 2; shot before
DISPLAY=$DISP xdotool windowsize $W 1000 850;      sleep 2   # tablet
DISPLAY=$DISP xdotool windowsize $W  800 850;      sleep 2   # phone
DISPLAY=$DISP xdotool windowsize $W 1350 850;      sleep 2; shot after
kill $APP $XVFB 2>/dev/null

python3 -c "
from PIL import Image, ImageChops
a, b = Image.open('$WORK/before.png'), Image.open('$WORK/after.png')
if ImageChops.difference(a, b).getbbox() is None:
    print('PASS: selection survived the breakpoints')
else:
    a.save('/tmp/bp-before.png'); b.save('/tmp/bp-after.png')
    raise SystemExit('FAIL: sidebar moved — see /tmp/bp-before.png, /tmp/bp-after.png')
"
