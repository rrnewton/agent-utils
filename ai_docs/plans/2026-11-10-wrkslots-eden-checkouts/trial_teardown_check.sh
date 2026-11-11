#!/bin/sh
# trial_teardown_check.sh TRIAL_MOUNT TARGET...
# Post-remove proof, read-only: the trial mount must be unlisted,
# no recorded target may remain, and this script only reports.
# Orphaned-byte comparison needs the before/after eden du JSON the
# caller captured with trial_meters.sh at both instants.
set -eu
trial_mount=$1; shift
fail=0
if eden list 2>/dev/null | grep -qx "$trial_mount"; then
  echo "FAIL: trial mount still listed: $trial_mount"; fail=1
else
  echo "ok: trial mount unlisted"
fi
for target in "$@"; do
  if [ -e "$target" ]; then
    echo "FAIL: redirection target still present: $target"; fail=1
  else
    echo "ok: target gone: $target"
  fi
done
if grep -q "$trial_mount" /proc/self/mountinfo; then
  echo "FAIL: trial mount still in mountinfo"; fail=1
else
  echo "ok: no mountinfo entry for trial mount"
fi
exit "$fail"
