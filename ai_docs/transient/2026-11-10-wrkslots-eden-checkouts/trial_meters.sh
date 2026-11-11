#!/bin/sh
# trial_meters.sh IMAGE_MOUNT TRIAL_MOUNT [EXISTING_MOUNT...]
# One instant of trial meters, all read-only. Never walks an Eden
# mount; usage comes from eden du/stats and btrfs accounting only.
set -eu
image_mount=$1; trial_mount=$2; shift 2
echo "== instant $(date -Is) =="
echo "-- btrfs /data --"
btrfs filesystem usage /data 2>/dev/null | grep -E "Metadata|Data,single|Device unallocated|Free \(estimated\)" || true
echo "-- btrfs image --"
btrfs filesystem usage "$image_mount" 2>/dev/null | grep -E "Metadata|Data,single|Device unallocated|Free \(estimated\)" || true
echo "-- qgroups --"
btrfs qgroup show "$image_mount" 2>/dev/null || true
echo "-- eden list --"
eden list || true
for mount in "$trial_mount" "$@"; do
  echo "-- eden du $mount --"
  eden du --json "$mount" 2>/dev/null || echo "unavailable: $mount"
  echo "-- eden info $mount --"
  eden info "$mount" 2>/dev/null || echo "unavailable: $mount"
done
echo "-- eden stats --"
eden stats 2>/dev/null | head -n 30 || true
echo "-- memory --"
awk '/^MemAvailable:/ {print}' /proc/meminfo
