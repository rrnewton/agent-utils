# Terminal multiplexer backends

This Rust library is the backend-neutral terminal-multiplexer contract that
agent tooling talks to, so no caller has to name a concrete multiplexer when
all it wants is a pane to read and type into.

Two traits make up the contract. `Multiplexer` owns the containers:
workspaces (tmux sessions), tabs (tmux windows), and panes — list, create,
focus, rename, and close. `PaneTerminal` owns one pane's surface: reading its
screen with scrollback, pasting literal text, sending named keys, submitting
a prompt, and reporting its working directory, title, and process identity.
Identifiers (`WorkspaceId`, `TabId`, `PaneId`) are opaque strings; a Herdr
pane id and a tmux pane id flow through the same code uninterpreted.

Two backends ship in the crate. `HerdrBackend` drives the Herdr CLI with the
same argument shapes agentctl has always used, including its native
per-agent status. `TmuxBackend` is a first-class second backend built on
`list-panes`, `capture-pane`, `load-buffer`/`paste-buffer`, `send-keys`,
and `new-window`; it has no native agent status, so its pane status is
`Unknown` and callers infer readiness from the screen, exactly as they do
for any pane whose backend reports no status. `FakeBackend` is an in-memory
backend for tests. External commands go through the `CommandRunner` seam,
so the Herdr and tmux argument shapes are unit-tested without a live server.

Backend selection is explicit, with detection as the fallback:
`BackendConfig` names a `BackendKind` (`Herdr` or `Tmux`), and when no kind
is named `detect_kind` prefers Herdr when its executable is on `PATH` and
falls back to tmux when only tmux exists. `open_backend` turns a config
into a `Box<dyn TermBackend>`.
