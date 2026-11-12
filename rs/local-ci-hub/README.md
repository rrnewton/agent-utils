# local-ci-hub (Rust)

Rust successor to the Python local-ci-hub ledger core (#1365
rust-ci-hub-ledger). One storage-agnostic core — typed events, one fold, one
qualifying predicate, one canonical digest — behind a fan-out `Store`
contract with one read authority per deployment, a disposable SQLite index,
and the existing six-object CLI.

M1 scope: typed core events with a refusing union and fold, the versioned
SQLite index with a rebuild-from-authority test, the `Store` trait with the
git least-common-denominator connector, read-only `receipt show`, and the
versioned `ci-hub-store-wire/v1` contract (implementation in M2). Python
remains the primary edition until parity and shadow gates pass; this crate is
reached with `cargo run -p local-ci-hub` meanwhile.

See `common/docs/local-ci-hub/` for operator documentation and the wire
contract.
