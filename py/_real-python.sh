# _real-python.sh — sourced by the py/bin wrappers; never executed directly.
#
# Why this exists (#1964 wrkslots-full-status-hang-ignores-sigterm): on
# hosts where `python3` is the fbpython launcher, the launcher forks the
# real interpreter as its child and waits in wait4 with SIGTERM caught.
# A supervisor that signals the process it started (`timeout`, systemd)
# signals only the launcher, never the worker, so a hung or finished
# worker can leave the launcher waiting forever. These wrappers therefore
# make the long-running process the real interpreter itself: resolve it
# once (the launcher runs only for that one short-lived probe), cache the
# answer per host, then exec the real interpreter directly. Signals from
# timeout or systemd then reach the process doing the work.
#
# Resolution honours ${AGENT_UTILS_PYTHON:-python3} as the command to
# resolve. The cache lives in a private state file keyed by the resolved
# launcher's path and mtime: a replaced launcher re-resolves. When the
# interpreter cannot be resolved, the wrappers fall back to plain
# `python3`, exactly the pre-#1964 behaviour. No host-specific path is
# hard-coded here.
#
# The sourcing wrapper sets _au_dir to the wrapper's directory (py/bin)
# before sourcing; this helper prepends that directory's parent (py/) to
# PYTHONPATH so `python -m <package>` finds the source checkout from any
# directory. This file lives in py/, not py/bin/, so the editions
# inventory never mistakes it for a tool entrypoint.

_au_resolve_real_python() {
    _au_cmd=${AGENT_UTILS_PYTHON:-python3}
    _au_launcher=$(command -v -- "$_au_cmd" 2>/dev/null) || _au_launcher=""
    if [ -n "$_au_launcher" ]; then
        _au_launcher_real=$(readlink -f "$_au_launcher" 2>/dev/null) || _au_launcher_real="$_au_launcher"
    else
        _au_launcher_real=""
    fi
    _au_mtime=""
    if [ -n "$_au_launcher_real" ]; then
        _au_mtime=$(stat -c %Y "$_au_launcher_real" 2>/dev/null || stat -f %m "$_au_launcher_real" 2>/dev/null) || _au_mtime=""
    fi
    _au_state=${XDG_STATE_HOME:-${HOME:-/tmp}/.local/state}/agent-utils
    _au_cache="$_au_state/real-python.cache"
    if [ -n "$_au_launcher_real" ] && [ -n "$_au_mtime" ] && [ -f "$_au_cache" ]; then
        _au_cached_launcher=$(sed -n '1p' "$_au_cache" 2>/dev/null) || _au_cached_launcher=""
        _au_cached_mtime=$(sed -n '2p' "$_au_cache" 2>/dev/null) || _au_cached_mtime=""
        _au_cached_real=$(sed -n '3p' "$_au_cache" 2>/dev/null) || _au_cached_real=""
        if [ "$_au_cached_launcher" = "$_au_launcher_real" ] && [ "$_au_cached_mtime" = "$_au_mtime" ] && [ -n "$_au_cached_real" ] && [ -x "$_au_cached_real" ]; then
            printf '%s\n' "$_au_cached_real"
            return 0
        fi
    fi
    _au_real=$("$_au_cmd" -c 'import sys; print(sys.executable)' 2>/dev/null) || _au_real=""
    if [ -n "$_au_real" ] && [ -x "$_au_real" ] && [ -n "$_au_launcher_real" ] && [ -n "$_au_mtime" ]; then
        (
            umask 077
            mkdir -p "$_au_state" 2>/dev/null || exit 0
            printf '%s\n%s\n%s\n' "$_au_launcher_real" "$_au_mtime" "$_au_real" > "$_au_cache.tmp" 2>/dev/null || exit 0
            mv "$_au_cache.tmp" "$_au_cache" 2>/dev/null || rm -f "$_au_cache.tmp"
        )
    fi
    printf '%s\n' "$_au_real"
}

# exec_real_python_module MODULE [ARGS...] — replace this shell with the
# real interpreter running the module. Falls back to plain python3 when
# resolution produced nothing usable.
exec_real_python_module() {
    _au_module=$1
    shift
    _au_real=$(_au_resolve_real_python)
    PYTHONPATH="${_au_dir}/..${PYTHONPATH:+:${PYTHONPATH}}"
    export PYTHONPATH
    if [ -n "$_au_real" ] && [ -x "$_au_real" ]; then
        exec "$_au_real" -m "$_au_module" "$@"
    fi
    exec python3 -m "$_au_module" "$@"
}

# exec_real_python_call MODULE FUNCTION [ARGS...] — like the above, for
# entry points whose module must be called by name to keep their argv0
# contract (herdr-chat: agentctl.chat.main, not the module's __main__
# fallback).
exec_real_python_call() {
    _au_module=$1
    _au_function=$2
    shift 2
    _au_real=$(_au_resolve_real_python)
    PYTHONPATH="${_au_dir}/..${PYTHONPATH:+:${PYTHONPATH}}"
    export PYTHONPATH
    if [ -n "$_au_real" ] && [ -x "$_au_real" ]; then
        exec "$_au_real" -c 'import sys; _m, _f = sys.argv[1], sys.argv[2]; del sys.argv[1:3]; import importlib; sys.exit(getattr(importlib.import_module(_m), _f)())' "$_au_module" "$_au_function" "$@"
    fi
    exec python3 -c 'import sys; _m, _f = sys.argv[1], sys.argv[2]; del sys.argv[1:3]; import importlib; sys.exit(getattr(importlib.import_module(_m), _f)())' "$_au_module" "$_au_function" "$@"
}
