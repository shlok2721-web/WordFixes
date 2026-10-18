#!/bin/bash
# Renders the app on a private offscreen X display and captures it.
# Never touches the real desktop.
set -u
OUT="${1:?usage: shoot.sh OUTPUT.png [WIDTHxHEIGHT]}"
GEO="${2:-1280x800}"
DISP=":99"
SP="${TMPDIR:-/tmp}"
APP="$(dirname "$0")/../target/debug/notes"

pkill -f "Xvfb $DISP" 2>/dev/null; sleep 0.3
Xvfb "$DISP" -screen 0 "${GEO}x24" >/dev/null 2>&1 &
XVFB=$!
sleep 1

DISPLAY="$DISP" GDK_BACKEND=x11 "$APP" > "$SP/offscreen.log" 2>&1 &
APPPID=$!
sleep 4

python3 -c "
from PIL import ImageGrab
img = ImageGrab.grab(xdisplay='$DISP')
img.save('$OUT')
print('captured', img.size)
"
kill $APPPID 2>/dev/null
kill $XVFB 2>/dev/null
