//! Synthetic expansion fixtures: stored codec lines and evidence auth.

use local_ci_hub::digest::sha256_of;
use local_ci_hub::event::Refusal;
use local_ci_hub::expand::{expand_line, read_shard_text, EvidenceSource, NoEvidence};
use local_ci_hub::pyjson::PyValue;
use std::cell::Cell;
use std::io::Write;
use std::process::{Command, Stdio};

struct CountingEvidence {
    calls: Cell<usize>,
    compressed: Vec<u8>,
}

impl EvidenceSource for CountingEvidence {
    fn read_compressed(&self, _locator: &str, _max_bytes: u64) -> Result<Vec<u8>, Refusal> {
        self.calls.set(self.calls.get() + 1);
        Ok(self.compressed.clone())
    }
}

const ENVELOPE: &str =
    "\"schema\":\"validate-ledger/v1\",\"event_id\":\"e1\",\"event_type\":\"run.result\",\
\"emitted_at\":\"2026-11-01T00:00:00Z\",\"team\":\"team-a\",\"host\":\"host-1\",\"run_id\":\"r1\",\
\"producer\":{\"source\":\"observed\",\"tool\":\"fixture\",\"tool_version\":\"1\"},\
\"commit\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"outcome\":\"pass\"";

#[test]
fn ordinary_line_passes_through() {
    let line = format!("{{{ENVELOPE}}}");
    let event = expand_line(&line, &NoEvidence).unwrap();
    assert_eq!(event.get_str("event_id"), Some("e1"));
}

#[test]
fn stored_duplicate_key_refused() {
    let line = format!(
        "{{{ENVELOPE},\"event_id\":\"e1\",\"legacy_row_codec\":{{\"codec\":\"cell-dedup/v1\"}}}}"
    );
    assert!(expand_line(&line, &NoEvidence).is_err());
}

#[test]
fn stored_integer_tokens_refused() {
    let codec = "\"legacy_row_codec\":{\"codec\":\"cell-dedup/v1\",\"comparisons\":[],\
                 \"evidence_binding_absent\":[],\"selected_from_cells\":false,\"census_cells_indexed\":false}";
    for bad in ["-0", "18446744073709551616", "-9223372036854775809"] {
        let line = [
            "{",
            ENVELOPE,
            ",",
            codec,
            ",\"legacy_row\":{\"cell_results\":{\"cells\":[],\"n\":",
            bad,
            "}}}",
        ]
        .concat();
        assert!(
            expand_line(&line, &NoEvidence).is_err(),
            "token {bad} must refuse"
        );
    }
    // Float tokens are exempt from the integer rule.
    let line = [
        "{",
        ENVELOPE,
        ",",
        codec,
        ",\"legacy_row\":{\"cell_results\":{\"cells\":[],\"n\":1e999}}}",
    ]
    .concat();
    // 1e999 parses to infinity in Python semantics; expansion itself succeeds.
    assert!(expand_line(&line, &NoEvidence).is_ok());
}

#[test]
fn cell_dedup_substitutes_and_derives() {
    let line = format!(
        "{{{ENVELOPE},\"legacy_row_codec\":{{\"codec\":\"cell-dedup/v1\",\
         \"comparisons\":[{{\"verdict\":\"same\"}}],\"evidence_binding_absent\":[],\
         \"selected_from_cells\":true,\"census_cells_indexed\":false}},\
         \"legacy_row\":{{\"cell_results\":{{\"run_id\":\"r1\",\"tree\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\",\"cells\":\
         [{{\"backend\":\"\",\"category\":\"unit\",\"lane\":\"main\",\"mode\":\"naked\",\"test\":\"t1\",\
           \"selected_attempt\":1,\"cell_verdict\":{{\"comparison\":0}}}}]}}}}}}"
    );
    let event = expand_line(&line, &NoEvidence).unwrap();
    assert!(!event.value.contains_key("legacy_row_codec"));
    let row = event.get("legacy_row").unwrap();
    let cells = row.get("cell_results").unwrap().get("cells").unwrap();
    let PyValue::Array(cells) = cells else {
        panic!("cells array")
    };
    let comparison = cells[0]
        .get("cell_verdict")
        .unwrap()
        .get("comparison")
        .unwrap();
    assert_eq!(
        comparison.get("verdict").and_then(PyValue::as_str),
        Some("same")
    );
    let binding = cells[0].get("evidence_binding").expect("derived binding");
    assert_eq!(
        binding.get("series_cell").and_then(PyValue::as_str),
        Some("t1/naked/native")
    );
    assert_eq!(
        binding.get("tree").and_then(PyValue::as_str),
        Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
    );
    let selected = row.get("cell_results").unwrap().get("selected").unwrap();
    let PyValue::Array(selected) = selected else {
        panic!("selected array")
    };
    assert_eq!(selected.len(), 1);
    assert_eq!(
        selected[0].get("test").and_then(PyValue::as_str),
        Some("t1")
    );
}

fn row_ref_line(
    sha256: &str,
    bytes: usize,
    compressed_sha256: &str,
    compressed_bytes: usize,
) -> String {
    format!(
        "{{{ENVELOPE},\"legacy_row_codec\":{{\"codec\":\"row-ref/v1\",\"sha256\":\"{sha256}\",\
         \"bytes\":{bytes},\"locator\":\"run-evidence/host-1/{sha256}.json.zst\",\
         \"compressed_sha256\":\"{compressed_sha256}\",\"compressed_bytes\":{compressed_bytes}}}}}"
    )
}

#[test]
fn row_ref_caps_refuse_before_fetch() {
    let sha = "a".repeat(64);
    let evidence = CountingEvidence {
        calls: Cell::new(0),
        compressed: Vec::new(),
    };
    let line = row_ref_line(&sha, 48 * 1024 * 1024 + 1, &sha, 10);
    assert!(expand_line(&line, &evidence).is_err());
    assert_eq!(evidence.calls.get(), 0, "caps must refuse before any fetch");
}

fn zstd_compress(data: &[u8]) -> Option<Vec<u8>> {
    let mut child = Command::new("zstd")
        .args(["--quiet", "--compress", "--stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(data).ok()?;
    let output = child.wait_with_output().ok()?;
    if output.status.success() {
        Some(output.stdout)
    } else {
        None
    }
}

#[test]
fn row_ref_happy_path_and_hash_refusals() {
    let raw = b"{\"run_id\":\"r1\",\"commit\":\"c1\",\"result\":\"pass\"}".to_vec();
    let Some(compressed) = zstd_compress(&raw) else {
        return; // zstd unavailable in this environment; cap test above still covers pre-fetch refusals.
    };
    let sha = sha256_of(&raw);
    let compressed_sha = sha256_of(&compressed);
    let evidence = CountingEvidence {
        calls: Cell::new(0),
        compressed: compressed.clone(),
    };
    let line = row_ref_line(&sha, raw.len(), &compressed_sha, compressed.len());
    let event = expand_line(&line, &evidence).unwrap();
    assert_eq!(evidence.calls.get(), 1);
    assert_eq!(
        event
            .get("legacy_row")
            .unwrap()
            .get("result")
            .and_then(PyValue::as_str),
        Some("pass")
    );

    // Wrong decompressed hash refuses.
    let bad_line = row_ref_line(
        &"b".repeat(64),
        raw.len(),
        &compressed_sha,
        compressed.len(),
    );
    let evidence = CountingEvidence {
        calls: Cell::new(0),
        compressed,
    };
    // Locator hex must equal sha256, so use the bad sha consistently: the
    // decompressed bytes then fail the row hash check... locator carries the
    // bad sha, fetch still returns the good object in this fake.
    assert!(expand_line(&bad_line, &evidence).is_err());
}

#[test]
fn truncated_shard_refused_and_splitlines() {
    let line = format!("{{{ENVELOPE}}}");
    assert!(
        read_shard_text(&line, &NoEvidence).is_err(),
        "missing final newline refuses"
    );
    let text = format!("{line}\n\n{line}\n");
    // Two copies of one event in one shard read fine here; union dedupes later.
    let events = read_shard_text(&text, &NoEvidence).unwrap();
    assert_eq!(events.len(), 2);
    // U+2028 inside a string splits the line (Python splitlines), refusing parse.
    let split = "{\"a\":\"x\u{2028}y\"}\n".to_string();
    assert!(read_shard_text(&split, &NoEvidence).is_err());
}
