#!/usr/bin/env bash
# Copy agent harness state into one durable, non-deleting archive per machine.

set -Eeuo pipefail
umask 077

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
readonly SCRIPT_DIR
readonly ARCHIVE_DIR="$SCRIPT_DIR"
readonly MACHINES_FILE="$SCRIPT_DIR/machines.tsv"
readonly ROOTS_FILE="$SCRIPT_DIR/log_roots.tsv"
readonly RUNS_DIR="$SCRIPT_DIR/_fetch_runs"
readonly STATE_DIR="$SCRIPT_DIR/_fetch_state"
readonly LOCK_FILE="$STATE_DIR/fetch.lock"

mode="fetch"
declare -a selected_machines=()
declare -a selected_roots=()
run_id=""
run_dir=""
results_file=""
log_file=""
started_utc=""
overall_status="not_started"
expected_operations=0
attempted_operations=0

usage() {
  cat <<'EOF'
Usage: fetch_agent_logs.sh [OPTIONS]

Fetch every configured log root from every configured machine. The current
machine is copied locally; other machines are read over SSH. Source files are
never changed, and destination files absent from a source are never deleted.

Options:
  --dry-run             Run rsync with --dry-run; do not change archived logs.
  --check-sources       Check source reachability/presence without rsync.
  --machine SHORT_NAME  Limit to one machine; repeat to select several.
  --root ARCHIVE_NAME   Limit to one root (for example .codex); repeatable.
  --list                Print the configured machine/root matrix and exit.
  -h, --help            Show this help.

Examples:
  ./fetch_agent_logs.sh --check-sources
  ./fetch_agent_logs.sh --dry-run --machine devbig176 --root .codex
  ./fetch_agent_logs.sh
EOF
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 2
}

utc_now() {
  date -u +%Y-%m-%dT%H:%M:%SZ
}

is_selected() {
  local candidate="$1"
  shift
  local selected
  if (($# == 0)); then
    return 0
  fi
  for selected in "$@"; do
    if [[ "$candidate" == "$selected" ]]; then
      return 0
    fi
  done
  return 1
}

validate_short_component() {
  local label="$1"
  local value="$2"
  [[ "$value" =~ ^[A-Za-z0-9._-]+$ ]] ||
    die "$label must be one safe path component: $value"
  [[ "$value" != "." && "$value" != ".." ]] ||
    die "$label may not be '.' or '..'"
}

validate_relative_source() {
  local value="$1"
  [[ "$value" =~ ^[A-Za-z0-9._+@/-]+$ ]] ||
    die "unsafe source path in $ROOTS_FILE: $value"
  [[ "$value" != /* && "$value" != ".." && "$value" != ../* &&
    "$value" != */../* && "$value" != */.. ]] ||
    die "source path must remain below the configured home: $value"
}

read_machine_rows() {
  local short_name host remote_home extra
  while IFS=$'\t' read -r short_name host remote_home extra; do
    [[ -z "$short_name" || "$short_name" == \#* ]] && continue
    [[ -z "${extra:-}" ]] || die "too many columns in $MACHINES_FILE"
    validate_short_component "machine short name" "$short_name"
    [[ "$host" =~ ^[A-Za-z0-9.-]+$ ]] || die "unsafe host: $host"
    [[ "$remote_home" == /* && "$remote_home" != *$'\n'* ]] ||
      die "machine home must be absolute: $remote_home"
    printf '%s\t%s\t%s\n' "$short_name" "$host" "$remote_home"
  done <"$MACHINES_FILE"
}

read_root_rows() {
  local machine_scope archive_name source_path requirement extra
  while IFS=$'\t' read -r machine_scope archive_name source_path requirement extra; do
    [[ -z "$machine_scope" || "$machine_scope" == \#* ]] && continue
    [[ -z "${extra:-}" ]] || die "too many columns in $ROOTS_FILE"
    if [[ "$machine_scope" != "*" ]]; then
      validate_short_component "root machine scope" "$machine_scope"
    fi
    validate_short_component "archive name" "$archive_name"
    validate_relative_source "$source_path"
    [[ "$requirement" == "required" || "$requirement" == "optional" ]] ||
      die "requirement must be required or optional for $archive_name"
    printf '%s\t%s\t%s\t%s\n' \
      "$machine_scope" "$archive_name" "$source_path" "$requirement"
  done <"$ROOTS_FILE"
}

assert_selectors_exist() {
  local requested found row_name machine_scope _rest
  for requested in "${selected_machines[@]}"; do
    found=false
    while IFS=$'\t' read -r row_name _rest; do
      [[ "$row_name" == "$requested" ]] && found=true
    done < <(read_machine_rows)
    [[ "$found" == true ]] || die "unknown machine: $requested"
  done
  for requested in "${selected_roots[@]}"; do
    found=false
    while IFS=$'\t' read -r machine_scope row_name _rest; do
      [[ "$row_name" == "$requested" ]] && found=true
    done < <(read_root_rows)
    [[ "$found" == true ]] || die "unknown root: $requested"
  done
}

validate_root_scopes() {
  local machine_scope _rest known_machine row_name _machine_rest
  while IFS=$'\t' read -r machine_scope _rest; do
    [[ "$machine_scope" == "*" ]] && continue
    known_machine=false
    while IFS=$'\t' read -r row_name _machine_rest; do
      [[ "$row_name" == "$machine_scope" ]] && known_machine=true
    done < <(read_machine_rows)
    [[ "$known_machine" == true ]] ||
      die "unknown machine scope in $ROOTS_FILE: $machine_scope"
  done < <(read_root_rows)
}

validate_unique_destinations() {
  local short_name host remote_home machine_scope archive_name source_path requirement key
  declare -A seen=()
  while IFS=$'\t' read -r short_name host remote_home; do
    while IFS=$'\t' read -r machine_scope archive_name source_path requirement; do
      [[ "$machine_scope" == "*" || "$machine_scope" == "$short_name" ]] || continue
      key="$short_name/$archive_name"
      [[ -z "${seen[$key]+present}" ]] ||
        die "multiple root rows resolve to destination $key"
      seen[$key]=1
    done < <(read_root_rows)
  done < <(read_machine_rows)
}

count_selected_operations() {
  local total=0
  local short_name host remote_home machine_scope archive_name source_path requirement
  while IFS=$'\t' read -r short_name host remote_home; do
    is_selected "$short_name" "${selected_machines[@]}" || continue
    while IFS=$'\t' read -r machine_scope archive_name source_path requirement; do
      [[ "$machine_scope" == "*" || "$machine_scope" == "$short_name" ]] || continue
      is_selected "$archive_name" "${selected_roots[@]}" || continue
      ((total += 1))
    done < <(read_root_rows)
  done < <(read_machine_rows)
  printf '%s\n' "$total"
}

list_configuration() {
  local short_name host remote_home machine_scope archive_name source_path requirement
  printf 'MACHINES\n'
  printf 'short_name\thost\thome\n'
  read_machine_rows
  printf '\nLOG ROOTS\n'
  printf 'machine_scope\tarchive_name\tsource_path\trequirement\n'
  read_root_rows
  printf '\nFETCH MATRIX\n'
  printf 'machine\tarchive_name\tsource\tdestination\n'
  while IFS=$'\t' read -r short_name host remote_home; do
    while IFS=$'\t' read -r machine_scope archive_name source_path requirement; do
      [[ "$machine_scope" == "*" || "$machine_scope" == "$short_name" ]] || continue
      printf '%s\t%s\t%s:%s/%s\t%s/%s/%s\n' \
        "$short_name" "$archive_name" "$host" "$remote_home" \
        "$source_path" "$ARCHIVE_DIR" "$short_name" "$archive_name"
    done < <(read_root_rows)
  done < <(read_machine_rows)
}

discover_forwarded_ssh_agent() {
  local tmux_value candidate
  if [[ -n "${SSH_AUTH_SOCK:-}" && -S "$SSH_AUTH_SOCK" ]]; then
    return 0
  fi
  if ! command -v tmux >/dev/null 2>&1; then
    return 0
  fi
  tmux_value="$(tmux show-environment -g SSH_AUTH_SOCK 2>/dev/null || true)"
  if [[ "$tmux_value" == SSH_AUTH_SOCK=* ]]; then
    candidate="${tmux_value#SSH_AUTH_SOCK=}"
    if [[ -S "$candidate" ]]; then
      export SSH_AUTH_SOCK="$candidate"
      printf 'Using forwarded SSH agent from the tmux environment.\n'
    fi
  fi
}

is_local_machine() {
  local short_name="$1"
  local host="$2"
  [[ "$short_name" == "$(hostname -s)" || "$host" == "$(hostname -f)" ]]
}

shell_quote() {
  printf '%q' "$1"
}

probe_source() {
  local short_name="$1"
  local host="$2"
  local source="$3"
  local quoted_source remote_command rc
  if is_local_machine "$short_name" "$host"; then
    [[ -d "$source" ]]
    return
  fi
  quoted_source="$(shell_quote "$source")"
  remote_command="if test -d $quoted_source; then exit 0; else exit 44; fi"
  # -n matters here: this function runs inside a loop whose stdin contains the
  # remaining configuration rows. An interactive ssh would consume those rows.
  if ssh -n -o BatchMode=yes -o ConnectTimeout=20 -- "$host" "$remote_command"; then
    return 0
  else
    rc=$?
  fi
  if ((rc == 44)); then
    return 44
  fi
  return "$rc"
}

record_result() {
  local machine="$1"
  local archive_name="$2"
  local requirement="$3"
  local status="$4"
  local exit_code="$5"
  local began="$6"
  local ended="$7"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$machine" "$archive_name" "$requirement" "$status" "$exit_code" \
    "$began" "$ended" >>"$results_file"
}

finish_run() {
  local exit_code=$?
  local finished_utc
  trap - EXIT
  if [[ -n "$run_dir" && -d "$run_dir" ]]; then
    finished_utc="$(utc_now)"
    if ((exit_code == 0)); then
      overall_status="complete"
    elif [[ "$overall_status" == "running" ]]; then
      overall_status="failed"
    fi
    {
      printf 'finished_utc\t%s\n' "$finished_utc"
      printf 'status\t%s\n' "$overall_status"
      printf 'exit_code\t%s\n' "$exit_code"
      printf 'attempted_operations\t%s\n' "$attempted_operations"
    } >>"$run_dir/run.tsv"
    ln -sfn -- "$run_id" "$RUNS_DIR/latest"
    printf '\nRun %s: %s (exit %s)\nMetadata: %s\n' \
      "$run_id" "$overall_status" "$exit_code" "$run_dir"
  fi
  exit "$exit_code"
}

while (($# > 0)); do
  case "$1" in
    --dry-run)
      [[ "$mode" == "fetch" ]] || die "choose only one execution mode"
      mode="dry_run"
      shift
      ;;
    --check-sources)
      [[ "$mode" == "fetch" ]] || die "choose only one execution mode"
      mode="check_sources"
      shift
      ;;
    --machine)
      (($# >= 2)) || die "--machine needs a value"
      selected_machines+=("$2")
      shift 2
      ;;
    --root)
      (($# >= 2)) || die "--root needs a value"
      selected_roots+=("$2")
      shift 2
      ;;
    --list)
      list_configuration
      exit 0
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

[[ -r "$MACHINES_FILE" ]] || die "missing $MACHINES_FILE"
[[ -r "$ROOTS_FILE" ]] || die "missing $ROOTS_FILE"
[[ "$(stat -c %u -- "$ARCHIVE_DIR")" == "$(id -u)" ]] ||
  die "archive directory must be owned by the current user: $ARCHIVE_DIR"
command -v rsync >/dev/null 2>&1 || die "rsync is required"
command -v flock >/dev/null 2>&1 || die "flock is required"
assert_selectors_exist
validate_root_scopes
validate_unique_destinations
expected_operations="$(count_selected_operations)"
((expected_operations > 0)) || die "the selection contains no operations"

mkdir -p -- "$RUNS_DIR" "$STATE_DIR"
exec 9>"$LOCK_FILE"
flock -n 9 || die "another fetch is already running (lock: $LOCK_FILE)"

run_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"
run_dir="$RUNS_DIR/$run_id"
results_file="$run_dir/results.tsv"
log_file="$run_dir/fetch.log"
mkdir -p -- "$run_dir"
cp -- "$MACHINES_FILE" "$run_dir/machines.tsv"
cp -- "$ROOTS_FILE" "$run_dir/log_roots.tsv"
sha256sum -- "$0" >"$run_dir/script.sha256"
printf 'machine\tarchive_name\trequirement\tstatus\texit_code\tstarted_utc\tfinished_utc\n' \
  >"$results_file"
started_utc="$(utc_now)"
{
  printf 'run_id\t%s\n' "$run_id"
  printf 'started_utc\t%s\n' "$started_utc"
  printf 'mode\t%s\n' "$mode"
  printf 'executing_host\t%s\n' "$(hostname -f)"
  printf 'archive_dir\t%s\n' "$ARCHIVE_DIR"
  printf 'expected_operations\t%s\n' "$expected_operations"
} >"$run_dir/run.tsv"
overall_status="running"
trap finish_run EXIT
exec > >(tee -a "$log_file") 2>&1

discover_forwarded_ssh_agent
printf 'Run: %s\nMode: %s\nStarted: %s\nArchive: %s\n' \
  "$run_id" "$mode" "$started_utc" "$ARCHIVE_DIR"

readonly RSYNC_SSH="ssh -o BatchMode=yes -o ConnectTimeout=20 -o ServerAliveInterval=15 -o ServerAliveCountMax=4"

failures=0
short_name=""
host=""
remote_home=""
archive_name=""
source_path=""
requirement=""
machine_scope=""
while IFS=$'\t' read -r short_name host remote_home; do
  is_selected "$short_name" "${selected_machines[@]}" || continue
  mkdir -p -- "$ARCHIVE_DIR/$short_name"
  while IFS=$'\t' read -r machine_scope archive_name source_path requirement; do
    [[ "$machine_scope" == "*" || "$machine_scope" == "$short_name" ]] || continue
    is_selected "$archive_name" "${selected_roots[@]}" || continue
    ((attempted_operations += 1))
    source_absolute="${remote_home%/}/$source_path"
    destination="$ARCHIVE_DIR/$short_name/$archive_name"
    began="$(utc_now)"
    printf '\n[%s] %s %s <- %s:%s\n' \
      "$began" "$short_name" "$archive_name" "$host" "$source_absolute"

    probe_rc=0
    probe_source "$short_name" "$host" "$source_absolute" || probe_rc=$?
    if ((probe_rc != 0)); then
      ended="$(utc_now)"
      if ((probe_rc == 44)) || (is_local_machine "$short_name" "$host" && ((probe_rc == 1))); then
        if [[ "$requirement" == "optional" ]]; then
          printf 'Optional source is absent; skipping.\n'
          record_result "$short_name" "$archive_name" "$requirement" \
            "missing_optional" "$probe_rc" "$began" "$ended"
          continue
        fi
        printf 'Required source is absent.\n'
        record_result "$short_name" "$archive_name" "$requirement" \
          "missing_required" "$probe_rc" "$began" "$ended"
      else
        printf 'Could not reach or inspect source (exit %s).\n' "$probe_rc"
        record_result "$short_name" "$archive_name" "$requirement" \
          "probe_failed" "$probe_rc" "$began" "$ended"
      fi
      ((failures += 1))
      continue
    fi

    if [[ "$mode" == "check_sources" ]]; then
      ended="$(utc_now)"
      printf 'Source is present.\n'
      record_result "$short_name" "$archive_name" "$requirement" \
        "present" 0 "$began" "$ended"
      continue
    fi

    if is_local_machine "$short_name" "$host"; then
      rsync_source="${source_absolute%/}/"
    else
      rsync_source="${host}:${source_absolute%/}/"
    fi
    mkdir -p -- "$STATE_DIR/partials/$short_name/$archive_name"
    rsync_args=(
      --archive
      --human-readable
      --protect-args
      --partial
      "--partial-dir=$STATE_DIR/partials/$short_name/$archive_name"
    )
    if ! is_local_machine "$short_name" "$host"; then
      rsync_args+=("--rsh=$RSYNC_SSH")
    fi
    if [[ "$mode" == "dry_run" ]]; then
      rsync_args+=(--dry-run "--info=stats2")
    else
      rsync_args+=("--info=progress2,stats2")
    fi

    rsync_rc=0
    rsync "${rsync_args[@]}" -- "$rsync_source" "${destination%/}/" || rsync_rc=$?
    ended="$(utc_now)"
    if ((rsync_rc == 0)); then
      if [[ "$mode" == "dry_run" ]]; then
        result_status="dry_run_complete"
      else
        result_status="fetched"
      fi
      record_result "$short_name" "$archive_name" "$requirement" \
        "$result_status" 0 "$began" "$ended"
    else
      printf 'rsync failed with exit %s. Partial data is retained for resumption.\n' \
        "$rsync_rc"
      record_result "$short_name" "$archive_name" "$requirement" \
        "rsync_failed" "$rsync_rc" "$began" "$ended"
      ((failures += 1))
    fi
  done < <(read_root_rows)
done < <(read_machine_rows)

if ((attempted_operations != expected_operations)); then
  printf '\nInternal error: attempted %s of %s selected operations.\n' \
    "$attempted_operations" "$expected_operations"
  ((failures += 1))
fi
if ((failures > 0)); then
  overall_status="failed"
  printf '\nCompleted with %s failed required/probe/rsync operation(s).\n' "$failures"
  exit 1
fi
overall_status="complete"
printf '\nAll selected operations completed successfully.\n'
