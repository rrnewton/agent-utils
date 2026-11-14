//! Synthetic core parity fixtures: JSON identity, union, fold, and digest.
//! All data is invented for this crate; no downstream corpus lives here.

use local_ci_hub::digest::{event_digest, receipt_canonical_json, receipt_digest};
use local_ci_hub::event::{
    check_events, is_fixture_event, parse_line, producer_tool, union, Refusal,
};
use local_ci_hub::fold::{fold_rows, terminal_lines_sorted};
use local_ci_hub::pyjson::{dumps_pairs, dumps_sorted, parse, py_float_repr, PyValue};
use std::collections::BTreeSet;

fn event(line: &str) -> local_ci_hub::event::RawEvent {
    parse_line(line).expect("fixture line parses")
}

fn result_event(event_id: &str, run_id: &str, emitted_at: &str, row: &str) -> String {
    format!(
        "{{\"schema\":\"validate-ledger/v1\",\"event_id\":\"{event_id}\",\"event_type\":\"run.result\",\
         \"emitted_at\":\"{emitted_at}\",\"team\":\"team-a\",\"host\":\"host-1\",\"run_id\":\"{run_id}\",\
         \"producer\":{{\"source\":\"observed\",\"tool\":\"fixture\",\"tool_version\":\"1\"}},\
         \"commit\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"outcome\":\"pass\",\"legacy_row\":{row}}}"
    )
}

#[test]
fn float_repr_matches_python() {
    assert_eq!(py_float_repr(1.0), "1.0");
    assert_eq!(py_float_repr(-0.0), "-0.0");
    assert_eq!(py_float_repr(100.0), "100.0");
    assert_eq!(py_float_repr(1e15), "1000000000000000.0");
    assert_eq!(py_float_repr(1e16), "1e+16");
    assert_eq!(py_float_repr(0.0001), "0.0001");
    assert_eq!(py_float_repr(0.00001), "1e-05");
    assert_eq!(py_float_repr(1.5e-5), "1.5e-05");
    assert_eq!(py_float_repr(f64::NAN), "NaN");
    assert_eq!(py_float_repr(f64::INFINITY), "Infinity");
    assert_eq!(py_float_repr(f64::NEG_INFINITY), "-Infinity");
    assert_eq!(
        py_float_repr(4.3300783700297997e-13),
        "4.3300783700297997e-13"
    );
}

#[test]
fn identity_ignores_key_order_but_keeps_duplicates() {
    let a = parse("{\"b\":1,\"a\":2}").unwrap();
    let b = parse("{\"a\":2,\"b\":1}").unwrap();
    assert_eq!(dumps_pairs(&a), dumps_pairs(&b));
    let c = parse("{\"a\":1,\"a\":2}").unwrap();
    let d = parse("{\"a\":2,\"a\":1}").unwrap();
    assert_ne!(dumps_pairs(&c), dumps_pairs(&d));
    // Dict view is last-wins; sorted dump collapses duplicates.
    assert_eq!(dumps_sorted(&c), "{\"a\":2}");
    assert_eq!(dumps_pairs(&c), "{\"a\":1,\"a\":2}");
}

#[test]
fn big_integers_and_ascii_escapes() {
    let value = parse("{\"n\":10000000000000000000000000000,\"s\":\"café\"}").unwrap();
    assert_eq!(
        dumps_sorted(&value),
        "{\"n\":10000000000000000000000000000,\"s\":\"caf\\u00e9\"}"
    );
    let neg_zero = parse("-0").unwrap();
    assert_eq!(neg_zero, PyValue::Int("0".to_owned()));
}

#[test]
fn union_refuses_conflicting_bodies_and_collapses_copies() {
    let one = result_event("e1", "r1", "2026-11-01T00:00:00Z", "{\"commit\":\"c1\"}");
    let copy = one.clone();
    let kept = union(vec![event(&one), event(&copy)]).unwrap();
    assert_eq!(kept.len(), 1);

    let other = result_event("e1", "r1", "2026-11-01T00:00:00Z", "{\"commit\":\"c2\"}");
    let err = union(vec![event(&one), event(&other)]).unwrap_err();
    assert_eq!(
        err,
        Refusal::ConflictingEventId {
            event_id: "e1".to_owned()
        }
    );
    assert_eq!(err.class(), "conflicting-event-id");
}

#[test]
fn union_orders_by_emitted_at_then_event_id() {
    let late = result_event("e2", "r2", "2026-11-02T00:00:00Z", "{}");
    let early = result_event("e1", "r1", "2026-11-01T00:00:00Z", "{}");
    let kept = union(vec![event(&late), event(&early)]).unwrap();
    assert_eq!(kept[0].get_str("event_id"), Some("e1"));
    assert_eq!(kept[1].get_str("event_id"), Some("e2"));
}

#[test]
fn annotate_metadata_only() {
    let ok = "{\"schema\":\"validate-ledger/v1\",\"event_id\":\"a1\",\"event_type\":\"run.annotate\",\
              \"emitted_at\":\"2026-11-01T00:00:00Z\",\"team\":\"team-a\",\"host\":\"host-1\",\"run_id\":\"r1\",\
              \"producer\":{\"source\":\"observed\",\"tool\":\"fixture\",\"tool_version\":\"1\"},\
              \"note\":{\"schema\":\"example/v1\",\"text\":\"hello\"}}";
    assert!(union(vec![event(ok)]).is_ok());

    let bad = ok.replace("\"note\"", "\"outcome\":\"pass\",\"note\"");
    let err = union(vec![event(&bad)]).unwrap_err();
    assert_eq!(err.class(), "annotate-changes-value");

    let bad_ref = ok.replace("\"note\"", "\"supersedes\":\"e1\",\"note\"");
    let err = union(vec![event(&bad_ref)]).unwrap_err();
    assert_eq!(err.class(), "annotate-changes-value");
}

#[test]
fn envelope_validation_catches_bad_shapes() {
    let good = result_event("e1", "r1", "2026-11-01T00:00:00Z", "{}");
    assert!(check_events(&[event(&good)]).is_ok());

    let dotted = good.replace("\"host\":\"host-1\"", "\"host\":\"host-1.example.com\"");
    assert!(check_events(&[event(&dotted)]).is_err());

    let bad_time = good.replace("2026-11-01T00:00:00Z", "2026-11-01T00:00:00+00:00");
    assert!(check_events(&[event(&bad_time)]).is_err());
}

#[test]
fn fold_substitutes_corrections_and_dedupes() {
    let base = result_event(
        "e1",
        "r1",
        "2026-11-01T00:00:00Z",
        "{\"run_id\":\"r1\",\"commit\":\"c1\",\"result\":\"fail\",\"host\":\"host-1\",\"finished_at\":\"2026-11-01T00:01:00Z\"}",
    );
    let correction = "{\"schema\":\"validate-ledger/v1\",\"event_id\":\"e2\",\"event_type\":\"run.correct\",\
         \"emitted_at\":\"2026-11-01T00:02:00Z\",\"team\":\"team-a\",\"host\":\"host-1\",\"run_id\":\"r1\",\
         \"producer\":{\"source\":\"observed\",\"tool\":\"fixture\",\"tool_version\":\"1\"},\
         \"supersedes\":\"e1\",\"reason\":\"wrong result recorded\",\
         \"legacy_row\":{\"run_id\":\"r1\",\"commit\":\"c1\",\"result\":\"pass\",\"host\":\"host-1\",\"finished_at\":\"2026-11-01T00:01:00Z\"}}".to_string();
    let events = union(vec![event(&base), event(&correction)]).unwrap();
    check_events(&events).unwrap();
    let outcome = fold_rows(&events, "/workspace").unwrap();
    assert_eq!(outcome.rows.len(), 1);
    assert_eq!(
        outcome.rows[0].get("result").and_then(PyValue::as_str),
        Some("pass")
    );
    let terminals = terminal_lines_sorted(&outcome.rows);
    assert_eq!(
        terminals,
        vec![
            "{\"commit\":\"c1\",\"finished_at\":\"2026-11-01T00:01:00Z\",\"host\":\"host-1\",\"result\":\"pass\",\"run_id\":\"r1\"}"
                .to_owned()
        ]
    );

    // Orphan correction refuses the whole fold.
    let orphan = correction.replace("\"supersedes\":\"e1\"", "\"supersedes\":\"missing\"");
    let events = union(vec![event(&base), event(&orphan)]).unwrap();
    assert!(fold_rows(&events, "/workspace").is_err());
}

#[test]
fn fold_restores_workspace_token_in_values_only() {
    let base = result_event(
        "e1",
        "r1",
        "2026-11-01T00:00:00Z",
        "{\"run_id\":\"r1\",\"log_file\":\"{{WORKSPACE_ROOT}}/logs/r1.log\"}",
    );
    let events = union(vec![event(&base)]).unwrap();
    let outcome = fold_rows(&events, "/repo").unwrap();
    assert_eq!(
        outcome.rows[0].get("log_file").and_then(PyValue::as_str),
        Some("/repo/logs/r1.log")
    );
}

#[test]
fn event_digest_is_sorted_ascii() {
    let value = parse("{\"b\":\"café\",\"a\":1}").unwrap();
    // sha256 of {"a":1,"b":"caf\u00e9"}
    assert_eq!(dumps_sorted(&value), "{\"a\":1,\"b\":\"caf\\u00e9\"}");
    assert_eq!(event_digest(&value).len(), 64);
    let reordered = parse("{\"a\":1,\"b\":\"café\"}").unwrap();
    assert_eq!(event_digest(&value), event_digest(&reordered));
}

#[test]
fn receipt_canonical_uses_struct_order() {
    let row = parse(
        "{\"result\":\"pass\",\"commit\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\
          \"schema_version\":5,\"real_seconds\":5.0,\"extra_key\":\"x\",\"gates\":[]}",
    )
    .unwrap();
    let canonical = receipt_canonical_json(&row).expect("row canonicalizes");
    // schema_version first, nulls for absent Options, extras last (sorted).
    assert!(
        canonical.starts_with("{\"schema_version\":5,\"run_id\":null,"),
        "{canonical}"
    );
    assert!(canonical.contains("\"real_seconds\":5.0"), "{canonical}");
    assert!(
        canonical.ends_with("\"gates\":[],\"extra_key\":\"x\"}"),
        "{canonical}"
    );
    assert_eq!(receipt_digest(&row).map(|d| d.len()), Some(64));

    // Wrong field type means no digest, never a different digest.
    let bad = parse("{\"executed_tests\":5.0}").unwrap();
    assert_eq!(receipt_digest(&bad), None);
    // Present-null build stamp is retained as an extension.
    let stamped = parse("{\"build_mode\":null}").unwrap();
    let canonical = receipt_canonical_json(&stamped).unwrap();
    assert!(canonical.contains("\"build_mode\":null"), "{canonical}");
}

#[test]
fn receipt_canonical_nested_typed_cell_results_use_struct_order() {
    // Synthetic nested record mirroring the typed evidence shape:
    // written key order is deliberately scrambled at every depth.
    let row = parse(
        "{\"schema_version\":7,\"run_id\":\"run-1\",\
          \"cell_results\":{\"cells\":[{\"backend\":\"b1\",\"mode\":\"m1\",\"test\":\"t1\",\
          \"category\":\"c1\",\"lane\":\"l1\",\"cell_verdict\":{\"bitwise_parity\":true,\
          \"compared_log_messages\":{\"right\":2,\"left\":1},\"comparison\":{\
          \"skip_detlog\":false,\"skip_commit\":true,\"ignore_lines\":false,\
          \"canonicalizations\":[],\"stripped_prefixes\":[\"p\"],\"exact_remainder\":true,\
          \"full_trace\":false,\"canonicalize_addresses\":true,\"strip_lines\":false,\
          \"log_scope\":\"info\",\"virtualize_time\":null,\"record_envelope\":\"all\",\
          \"compare_io_buffers\":true,\"compare_logs\":true,\"display_name\":\"Spec\",\
          \"strictness\":\"canonical\"},\"comparison_tier\":\"canonical-bitwise\",\
          \"state\":\"compared-and-matched\"}}],\
          \"selected\":[{\"test\":\"t1\",\"backend\":\"b1\",\"lane\":\"l1\",\"mode\":\"m1\",\"category\":\"c1\"}],\
          \"artifact\":{\"row_count\":3,\"sha256\":\"s\",\"path\":\"a/b.jsonl\"},\
          \"population_sha256\":\"pop\",\"recorded_count\":1,\"selected_count\":1,\
          \"source_tree_dirty\":false,\"hermit_sha\":\"hhhh\",\"run_id\":\"run-1\"},\
          \"coverage\":{\"artifact\":{\"path\":\"cov.json\",\"sha256\":\"cs\",\"row_count\":9},\
          \"planned_test_nodes\":20,\"executed_test_nodes\":20,\"zero_executed_nodes\":[],\"absent_nodes\":[]},\
          \"gates\":[{\"name\":\"g1\",\"result\":\"pass\",\"attempts\":[{\"attempt\":1,\"real_seconds\":1.5,\"nested\":{\"y\":1,\"x\":\"café\"}}]}]}",
    )
    .unwrap();
    let canonical = receipt_canonical_json(&row).expect("row canonicalizes");
    // Evidence struct order, artifact struct order, identity order.
    assert!(
        canonical.contains(
            "\"cell_results\":{\"run_id\":\"run-1\",\"hermit_sha\":\"hhhh\",\"source_tree_dirty\":false,\
             \"selected_count\":1,\"recorded_count\":1,\"population_sha256\":\"pop\",\
             \"artifact\":{\"path\":\"a/b.jsonl\",\"sha256\":\"s\",\"row_count\":3},\
             \"selected\":[{\"lane\":\"l1\",\"category\":\"c1\",\"test\":\"t1\",\"mode\":\"m1\",\"backend\":\"b1\"}]"
        ),
        "{canonical}"
    );
    // Coverage keeps only its four typed fields; the artifact sub-object
    // is dropped by the typed shape on both sides.
    assert!(
        canonical.contains(
            "\"coverage\":{\"planned_test_nodes\":20,\"executed_test_nodes\":20,\
             \"zero_executed_nodes\":[],\"absent_nodes\":[]}"
        ),
        "{canonical}"
    );
    // Gate attempts stay in the sorted extension area at every depth.
    assert!(
        canonical.contains(
            "\"attempts\":[{\"attempt\":1,\"nested\":{\"x\":\"café\",\"y\":1},\"real_seconds\":1.5}]"
        ),
        "{canonical}"
    );
}

#[test]
fn receipt_canonical_untyped_cell_results_keep_sorted_form() {
    // An unknown key defeats the typed shape: the whole value falls
    // back to the generic sorted-key form.
    let row = parse(
        "{\"cell_results\":{\"run_id\":\"run-1\",\"future_field\":{\"z\":1,\"a\":2}}}",
    )
    .unwrap();
    let canonical = receipt_canonical_json(&row).expect("row canonicalizes");
    assert!(
        canonical.contains(
            "\"cell_results\":{\"future_field\":{\"a\":2,\"z\":1},\"run_id\":\"run-1\"}"
        ),
        "{canonical}"
    );
}

#[test]
fn fixture_events_match_by_envelope_or_row_producer() {
    let producers: BTreeSet<String> = ["live-sync-check".to_owned()].into();
    let ordinary = result_event("e1", "r1", "2026-11-01T00:00:00Z", "{}");
    assert!(!is_fixture_event(&event(&ordinary), &producers));

    let envelope = ordinary.replace("\"tool\":\"fixture\"", "\"tool\":\"live-sync-check\"");
    assert!(is_fixture_event(&event(&envelope), &producers));

    let row_only = result_event(
        "e2",
        "r2",
        "2026-11-01T00:00:00Z",
        "{\"producer\":{\"tool\":\"live-sync-check\"}}",
    );
    assert!(is_fixture_event(&event(&row_only), &producers));

    let legacy_string_producer = parse("{\"producer\":\"live-sync-check\"}").unwrap();
    assert_eq!(
        producer_tool(legacy_string_producer.get("producer")),
        "live-sync-check"
    );

    let no_filters: BTreeSet<String> = BTreeSet::new();
    assert!(!is_fixture_event(&event(&envelope), &no_filters));
}
