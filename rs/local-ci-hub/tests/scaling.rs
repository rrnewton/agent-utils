//! Scaling guard for the streaming fold: peak live heap must track the
//! working set (distinct terminals), not the number of event copies fed.
//! The assertion is structural (counted allocation bytes), never
//! wall-clock or sampled RSS. Synthetic fixtures only.

use local_ci_hub::event::{parse_line, union};
use local_ci_hub::fold::{fold_rows, fold_rows_streaming};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let now = CURRENT.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(now, Ordering::SeqCst);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        CURRENT.fetch_sub(layout.size(), Ordering::SeqCst);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn result_line(event_id: &str, run_id: &str, emitted_at: &str, row: &str) -> String {
    format!(
        "{{\"schema\":\"validate-ledger/v1\",\"event_id\":\"{event_id}\",\"event_type\":\"run.result\",\
         \"emitted_at\":\"{emitted_at}\",\"team\":\"team-a\",\"host\":\"host-1\",\"run_id\":\"{run_id}\",\
         \"producer\":{{\"source\":\"observed\",\"tool\":\"fixture\",\"tool_version\":\"1\"}},\
         \"commit\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"outcome\":\"pass\",\"legacy_row\":{row}}}"
    )
}

fn scaled_line(index: usize) -> String {
    let pad = "x".repeat(6000);
    result_line(
        &format!("evt-{index:04}"),
        &format!("run-{index:04}"),
        "2026-11-01T00:00:00Z",
        &format!(
            "{{\"run_id\":\"run-{index:04}\",\"commit\":\"c{index}\",\"result\":\"pass\",\
             \"host\":\"host-1\",\"finished_at\":\"2026-11-01T00:01:00Z\",\"pad\":\"{pad}\"}}"
        ),
    )
}

fn peak_live_during(run: impl FnOnce()) -> usize {
    let start = CURRENT.load(Ordering::SeqCst);
    PEAK.store(start, Ordering::SeqCst);
    run();
    PEAK.load(Ordering::SeqCst).saturating_sub(start)
}

#[test]
fn streaming_fold_scales_with_working_set_not_input_copies() {
    // Byte parity with the slice fold on a correction history, with
    // duplicate copies interleaved (already in union order).
    let base = result_line(
        "e1",
        "r1",
        "2026-11-01T00:00:00Z",
        "{\"run_id\":\"r1\",\"commit\":\"c1\",\"result\":\"fail\",\"host\":\"host-1\",\"finished_at\":\"2026-11-01T00:01:00Z\"}",
    );
    let correction = "{\"schema\":\"validate-ledger/v1\",\"event_id\":\"e2\",\"event_type\":\"run.correct\",\
         \"emitted_at\":\"2026-11-01T00:02:00Z\",\"team\":\"team-a\",\"host\":\"host-1\",\"run_id\":\"r1\",\
         \"producer\":{\"source\":\"observed\",\"tool\":\"fixture\",\"tool_version\":\"1\"},\
         \"supersedes\":\"e1\",\"reason\":\"wrong result recorded\",\
         \"legacy_row\":{\"run_id\":\"r1\",\"commit\":\"c1\",\"result\":\"pass\",\"host\":\"host-1\",\"finished_at\":\"2026-11-01T00:01:00Z\"}}"
        .to_owned();
    let lines = [base.clone(), base, correction.clone(), correction];
    let slice_events = union(
        lines
            .iter()
            .map(|line| parse_line(line).expect("line parses"))
            .collect(),
    )
    .expect("union ok");
    let expected = fold_rows(&slice_events, "/workspace").expect("slice fold ok");
    let mut streamed: Vec<String> = Vec::new();
    let count = fold_rows_streaming(
        || {
            lines
                .iter()
                .map(|line| parse_line(line))
                .collect::<Vec<_>>()
                .into_iter()
        },
        "/workspace",
        |_row, line| streamed.push(line),
    )
    .expect("streaming fold ok");
    assert_eq!(count, expected.rows.len());
    assert_eq!(streamed, expected.lines);

    // Conflicting copies refuse in the streaming fold as in the union.
    let other = result_line(
        "e1",
        "r1",
        "2026-11-01T00:00:00Z",
        "{\"run_id\":\"r1\",\"commit\":\"c2\"}",
    );
    let conflict = [lines[0].clone(), other];
    assert!(fold_rows_streaming(
        || {
            conflict
                .iter()
                .map(|line| parse_line(line))
                .collect::<Vec<_>>()
                .into_iter()
        },
        "/workspace",
        |_row, _line| {},
    )
    .is_err());

    // N versus 4N event copies over a fixed distinct working set (150
    // terminals): peak live heap may wobble, never scale with copies.
    const DISTINCT: usize = 150;
    let measure = |repeats: usize| -> (usize, usize) {
        let mut emitted = 0usize;
        let peak = peak_live_during(|| {
            emitted = fold_rows_streaming(
                || (0..DISTINCT * repeats).map(move |n| parse_line(&scaled_line(n % DISTINCT))),
                "/workspace",
                |_row, _line| {},
            )
            .expect("scaled fold ok");
        });
        (peak, emitted)
    };
    let (peak_n, rows_n) = measure(1);
    let (peak_4n, rows_4n) = measure(4);
    eprintln!("scaling peaks: N={peak_n} bytes, 4N={peak_4n} bytes");
    assert_eq!(rows_n, DISTINCT);
    assert_eq!(rows_4n, DISTINCT);
    assert!(
        peak_4n <= peak_n.saturating_mul(2) + 1_000_000,
        "peak live heap scaled with input copies: N={peak_n} bytes, 4N={peak_4n} bytes"
    );
}
