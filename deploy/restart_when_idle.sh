#!/usr/bin/env bash
# Restarts the backend (and with it the game) when no plot is in use, so a long
# run's leaks in the Java process are cleared without cutting anyone off. The
# weekly timer (deploy/redfab-restart.timer) runs it; when a plot is in use it
# does nothing and says so, and the next week's run tries again.
set -u
port=${REDFAB_PORT:-8766}
status=$(curl -fsS --max-time 5 "http://127.0.0.1:$port/api/status") || { echo "no status: the backend is down or restarting; nothing to do"; exit 0; }
free=$(node -e 'const s = JSON.parse(process.argv[1]); console.log(s.plots.total - s.plots.free === 0 ? "idle" : `${s.plots.total - s.plots.free} in use`)' "$status")
if [ "$free" = idle ]; then
    echo "idle: restarting redfab-live"
    systemctl restart redfab-live
else
    echo "$free: not restarting"
fi
