#!/bin/sh
# Starts the bot's desktop: a 1280x800 screen, a window manager, the browser, and the live view
# (VNC on the box's own loopback, carried to Catbot as a websocket on port 6080).
export DISPLAY=:1
rm -f /tmp/.X1-lock /tmp/.X11-unix/X1
Xvfb :1 -screen 0 1280x800x24 -nolisten tcp &
for i in 1 2 3 4 5 6 7 8 9 10; do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.3; done
fluxbox >/dev/null 2>&1 &
x11vnc -display :1 -forever -shared -nopw -localhost -rfbport 5900 -quiet >/dev/null 2>&1 &
websockify 6080 localhost:5900 >/dev/null 2>&1 &
# A browser stopped with the box isn't a crash: no "Restore pages?" next time.
prefs="$HOME/.config/chromium/Default/Preferences"
[ -f "$prefs" ] && sed -i 's/"exited_cleanly":false/"exited_cleanly":true/; s/"exit_type":"[A-Za-z]*"/"exit_type":"Normal"/' "$prefs"
chromium --no-sandbox --test-type --no-first-run --no-default-browser-check --hide-crash-restore-bubble --start-maximized about:blank >/dev/null 2>&1 &
# Stopping the box closes the browser properly first.
trap 'pkill -TERM chromium; sleep 2; exit 0' TERM INT
sleep infinity &
wait $!
