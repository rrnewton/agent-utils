#!/usr/bin/env bash
# End-to-end: image slots whose mounts an unrelated private mount namespace has copied
# (#350 wrkslots-foreign-mount-ns), against real loop mounts.
#
#   e2e_foreign_namespace.sh
#
# A process that unshares its mount namespace while slot images are mounted keeps a copy of
# every slot mount; a host unmount does not reach it, so the copy keeps the loop device bound.
# This script starts such processes with `unshare -rm --propagation private` and checks that:
#
#   (a) remove succeeds while an idle copy exists, and afterwards no namespace mounts the slot,
#       no loop device holds its image, and the image directory is gone;
#   (b) remove and image unmount refuse while a process in that namespace works inside the slot;
#   (c) image mount after a host-only unmount reuses the device the copy holds: one attachment,
#       and the host mounts that device;
#   (d) an image attached to a second loop device (forced with losetup -f, then mount) is reported
#       by doctor as image-attached-twice, refused by plain remove, and removed with
#       --coordinator-authorized --discard-stale-image-copy;
#   (e) one slot whose mount point is not empty does not stop image mount from mounting the
#       others; it exits 3 and names that slot.
#
# Needs passwordless sudo, unprivileged user namespaces, git, and mkfs.ext4. Agent sandboxes
# usually refuse sudo and unshare, so the owner or a CI job with both runs this. The test
# directory (E2E_BASE, default /var/tmp) is removed at exit unless E2E_KEEP is set, and the
# script checks that no mount or loop device under it leaked.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
PYROOT=$(cd "$HERE/../.." && pwd)
skip() { echo "SKIP: $*"; exit 0; }
[ "$(id -u)" != 0 ] || skip "run as a regular user"
sudo -n true 2>/dev/null || skip "passwordless sudo -n is unavailable"
unshare -rm true 2>/dev/null || skip "no unprivileged user namespaces"
command -v mkfs.ext4 >/dev/null || skip "mkfs.ext4 is not installed"

PY=$(python3 -c 'import os, sys; print(os.path.realpath(sys.executable))')
BASE=$(mktemp -d "${E2E_BASE:-/var/tmp}/wrkslots-e2e-foreign-XXXXXX")
PROJECT=$BASE/project
export WRKSLOTS_IMAGE_BACKEND=kernel
W() { "$PY" "$PYROOT/wrkslots/__main__.py" --project-root "$PROJECT" "$@"; }
FOREIGN=()
loops() { for f in /sys/block/loop*/loop/backing_file; do [ -f "$f" ] && grep -lF "$BASE/" "$f"; done 2>/dev/null | sed 's,/sys/block/\(loop[0-9]*\)/.*,/dev/\1,' | sort; }
loops_for() { for f in /sys/block/loop*/loop/backing_file; do [ -f "$f" ] && grep -qxF "$1" "$f" && echo "$f"; done 2>/dev/null | sed 's,/sys/block/\(loop[0-9]*\)/.*,/dev/\1,' | sort; }
host_mounts() { awk -v b="$BASE/" 'index($5, b) == 1 {print $5}' /proc/self/mountinfo | sort; }
cleanup() {
  for pid in "${FOREIGN[@]}"; do kill "$pid" 2>/dev/null; done
  [ -f "$BASE/owner.pid" ] && kill "$(cat "$BASE/owner.pid")" 2>/dev/null
  sleep 1
  host_mounts | sort -r | while read -r target; do sudo -n umount "$target" 2>/dev/null; done
  for device in $(loops); do sudo -n losetup -d "$device" 2>/dev/null; done
  rm -rf "$BASE"
}
[ -n "${E2E_KEEP:-}" ] || trap cleanup EXIT
failures=0
check() { # check LABEL EXPECTED ACTUAL
  if [ "$2" = "$3" ]; then echo "PASS $1"; else echo "FAIL $1: expected '$2', got '$3'"; failures=$((failures + 1)); fi
}
contains() { # contains LABEL NEEDLE HAYSTACK
  case "$3" in *"$2"*) echo "PASS $1" ;; *) echo "FAIL $1: '$2' not in: $3"; failures=$((failures + 1)) ;; esac
}
LAST_FOREIGN=
foreign() { # foreign [DIRECTORY]: a process in a private copy of this mount namespace -> LAST_FOREIGN
  if [ -n "${1:-}" ]; then
    unshare -rm --propagation private sh -c "cd '$1' && exec sleep 600" &
  else
    unshare -rm --propagation private sleep 600 &
  fi
  LAST_FOREIGN=$!
  FOREIGN+=("$LAST_FOREIGN")
  sleep 1
}
lists() { grep -qF " $2 " "/proc/$1/mountinfo" 2>/dev/null && echo yes || echo no; }

set -e
git init -q --bare "$BASE/remote.git"
git clone -q "$BASE/remote.git" "$PROJECT/src" 2>/dev/null
git -C "$PROJECT/src" config user.email e2e@example.invalid
git -C "$PROJECT/src" config user.name e2e
echo seed > "$PROJECT/src/seed"
git -C "$PROJECT/src" add seed
git -C "$PROJECT/src" commit -qm seed
git -C "$PROJECT/src" push -q origin HEAD:main 2>/dev/null
mkdir -p "$PROJECT/tools"
cat > "$PROJECT/tools/liveness.py" <<'EOF'
#!/usr/bin/env python3
import os, sys
alive = os.path.exists(os.path.join(os.environ["WRKSLOTS_PROJECT_ROOT"], "owner-alive"))
print(f"{'alive' if alive else 'dead'} agent={sys.argv[1]} rc={1 if alive else 0}")
sys.exit(1 if alive else 0)
EOF
chmod +x "$PROJECT/tools/liveness.py"
"$PY" "$PYROOT/wrkslots/__main__.py" init "$PROJECT" --liveness-command tools/liveness.py \
  --heartbeat-ttl-seconds 1 --layout flat --slot-representation image --image-ceiling-gib 2 \
  --salvage-push-remote origin >/dev/null
sleep 3600 & echo $! > "$BASE/owner.pid"
OWNER=$(cat "$BASE/owner.pid")
make_slot() { # make_slot NAME: an image slot with one pushed commit
  touch "$PROJECT/owner-alive"
  W create "$1" --slot-type agent --coordinator-authorized --agent "agent-$1" --task "t-$1" \
    --purpose e2e --coordinator-pid $$ --owner-pid "$OWNER" --repo src=src --branch "src=e2e/$1" >/dev/null
  local tree=$PROJECT/worktrees/slots/$1
  echo "$1" > "$tree/work.txt"
  git -C "$tree" add work.txt
  git -C "$tree" -c user.email=e -c user.name=e commit -qm "$1"
  git -C "$tree" push -q origin "e2e/$1" 2>/dev/null
}
owner_gone() { rm -f "$PROJECT/owner-alive"; sleep 2; }
remove() { W remove "$1" --coordinator-pid $$ --expected-generation 1 "${@:2}"; }
set +e

echo "=== (a) an idle copy does not block removal, and removal releases it"
make_slot a
SLOT_A=$PROJECT/worktrees/slots/a
IMAGE_A=$PROJECT/worktrees/slot-images/agent/a/slot.img
foreign; PID_A=$LAST_FOREIGN
check "(a) the foreign namespace holds a copy" yes "$(lists "$PID_A" "$SLOT_A")"
owner_gone
remove a >/dev/null 2>"$BASE/a.err"
check "(a) remove succeeds with the copy present" 0 $?
check "(a) the foreign namespace no longer mounts the slot" no "$(lists "$PID_A" "$SLOT_A")"
check "(a) no loop device holds the image" "" "$(loops_for "$IMAGE_A")"
check "(a) the image directory is gone" no "$([ -e "$PROJECT/worktrees/slot-images/agent/a" ] && echo yes || echo no)"

echo "=== (b) a process working inside the copy still refuses"
make_slot b
SLOT_B=$PROJECT/worktrees/slots/b
foreign "$SLOT_B"; PID_B=$LAST_FOREIGN
owner_gone
OUT=$(remove b 2>&1); RC=$?
check "(b) remove refuses" 3 "$RC"
contains "(b) the refusal names the process" "$PID_B" "$OUT"
OUT=$(W image unmount b 2>&1); RC=$?
check "(b) image unmount refuses" 3 "$RC"
check "(b) the slot is still mounted on the host" yes "$(host_mounts | grep -qxF "$SLOT_B" && echo yes || echo no)"
kill "$PID_B"; sleep 1
remove b >/dev/null 2>&1
check "(b) remove succeeds once that process left" 0 $?

echo "=== (c) remounting after a host-only unmount reuses the held device"
make_slot c
SLOT_C=$PROJECT/worktrees/slots/c
IMAGE_C=$PROJECT/worktrees/slot-images/agent/c/slot.img
DEVICE_C=$(findmnt -no SOURCE "$SLOT_C")
foreign
sudo -n umount "$SLOT_C"
check "(c) the copy keeps the device bound" "$DEVICE_C" "$(loops_for "$IMAGE_C")"
OUT=$(W image mount 2>&1)
contains "(c) image mount says it reused the device" "reusing $DEVICE_C" "$OUT"
check "(c) exactly one device holds the image" "$DEVICE_C" "$(loops_for "$IMAGE_C")"
check "(c) the host mounts that device" "$DEVICE_C" "$(findmnt -no SOURCE "$SLOT_C")"

echo "=== (d) an image attached twice is reported and removed only on authority"
sudo -n umount "$SLOT_C"
SECOND=$(sudo -n losetup -f --show "$IMAGE_C")
sudo -n mount "$SECOND" "$SLOT_C"
check "(d) two devices hold the image" 2 "$(loops_for "$IMAGE_C" | wc -l)"
OUT=$(W doctor --format json 2>&1)
contains "(d) doctor reports image-attached-twice" "image-attached-twice" "$OUT"
owner_gone
OUT=$(remove c 2>&1); RC=$?
check "(d) plain remove refuses" 3 "$RC"
contains "(d) the refusal names the stale copy" "stale-image-copy=$DEVICE_C" "$OUT"
remove c --coordinator-authorized --discard-stale-image-copy >/dev/null 2>"$BASE/d.err"
check "(d) remove with the flag succeeds" 0 $?
sleep 2
check "(d) no loop device holds the image" "" "$(loops_for "$IMAGE_C")"

echo "=== (e) one unmountable slot does not stop the others"
make_slot e1
make_slot e2
sudo -n umount "$PROJECT/worktrees/slots/e1" 2>/dev/null
sudo -n umount "$PROJECT/worktrees/slots/e2" 2>/dev/null
echo stray > "$PROJECT/worktrees/slots/e1/stray"
OUT=$(W image mount 2>&1); RC=$?
check "(e) image mount exits 3" 3 "$RC"
contains "(e) it names the slot it could not mount" "agent/e1" "$OUT"
check "(e) the other slot is mounted" yes "$(host_mounts | grep -qxF "$PROJECT/worktrees/slots/e2" && echo yes || echo no)"
rm -f "$PROJECT/worktrees/slots/e1/stray"
W image mount >/dev/null 2>&1
owner_gone
remove e1 >/dev/null 2>&1
check "(e) e1 removed" 0 $?
remove e2 >/dev/null 2>&1
check "(e) e2 removed" 0 $?

for pid in "${FOREIGN[@]}"; do kill "$pid" 2>/dev/null; done
sleep 1
check "no mount under the test directory leaked" "" "$(host_mounts | grep -F "/worktrees/slots/")"
check "no loop device under the test directory leaked" "" "$(loops)"
if [ "$failures" -ne 0 ]; then echo "E2E FAIL: $failures check(s)"; exit 1; fi
echo "E2E PASS"
