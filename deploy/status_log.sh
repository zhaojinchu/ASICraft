#!/usr/bin/env bash
# One line: the time and the backend's /api/status, or the failure to get it.
# deploy/status_log.cron runs it every minute into /var/log/redfab/status.log.
set -u
port=${REDFAB_PORT:-8766}
stamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)
if status=$(curl -fsS --max-time 5 "http://127.0.0.1:$port/api/status" 2>&1); then
    printf '%s %s\n' "$stamp" "$status"
else
    printf '%s DOWN %s\n' "$stamp" "$status"
fi
