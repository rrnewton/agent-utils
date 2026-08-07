# Agent log archive fetcher

This is the source-controlled template for a private, non-deleting archive of
agent-harness state collected from several machines. It supports local and SSH
sources, selection/dry-run modes, resumable transfers, exclusive locking, and
an immutable per-invocation metadata ledger.

Install it into a private archive directory, then replace the example machine
inventory with real hosts:

```bash
archive_dir="$HOME/agent_logs_archive"
install -d -m 0700 "$archive_dir"
install -m 0755 scripts/agent-log-archive/fetch_agent_logs.sh "$archive_dir/"
install -m 0644 scripts/agent-log-archive/log_roots.tsv "$archive_dir/"
install -m 0644 scripts/agent-log-archive/machines.example.tsv "$archive_dir/machines.tsv"
```

Keep `machines.tsv` private when hostnames or home paths are sensitive. Check
and preview the complete matrix before the first real transfer:

```bash
cd "$HOME/agent_logs_archive"
./fetch_agent_logs.sh --list
./fetch_agent_logs.sh --check-sources
./fetch_agent_logs.sh --dry-run
./fetch_agent_logs.sh
```

The four required roots cover the observed Orc, Claude, and Codex transcript
stores. The optional roots cover per-user Gas Town/Beads state, alternate
Claude accounts, and Gas Town's default town location. A non-default town is a
machine-scoped row with a unique archive name:

```text
build02\tgas-town-main\twork/my-town\trequired
```

The destination never uses rsync deletion, so source-side rotation does not
remove old archive paths. Repeated fetches update the same live tree and
transfer only differences. This is not same-path file versioning, and live
SQLite databases are not guaranteed to be transactionally coherent across a
raw rsync. Downstream consumers should first make an immutable working snapshot
and use SQLite-aware backups or validation where needed.

Full harness trees can contain credentials, private prompts, and symlinks into
working trees. Keep the archive mode `0700`, do not publish it, and do not add
`--copy-links`.
