#!/usr/bin/env python3
"""`#532 shift-o-latency` in real Chromium at 393x852 (touch) and 1280x800.

The owner, 2026-10-26: "shift-o is slow - why? It's not terribly slow but it does take a couple seconds.
If I am folding up 4 messages in a row in a thread I thought I could spam the key four times and see them
responsively fold up." And of e: "the 'e' key and marking archived is one of those things that I most want
to be instantly responsive locally."

Against a fake API of twenty threads, one with twelve long replies and the rest with four, every thread
unpacked in the Threads view (Unpack all):

* Shift+O: eight presses 60 ms apart on the last reply fold the eight last replies, each one drawn
  folded, and the eight folds are one line. The gating verdict is that behaviour and nothing else:
  no millisecond budget decides it, so a hot host cannot fail it and a broken fold cannot pass it.
  The handler work is still measured and printed (the main thread's own CPU time for the key's
  dispatch, corrected by the layout reference bracketing each press, with the host's load sampled
  at each press), and `--benchmark` enforces the old budgets — `--cpu-ms` at the median and twice
  that for all but the largest quarter of presses (#905, #866's ~1.5x solo rule), the wall budget
  at 8x it — as an opt-in speed measurement run outside the gate.
* e: the same on two other threads' replies, four presses on each, each marked Done and drawn so.
* style: hiding one row restyles a few elements, not the page: under `--restyle-max` elements in the
  browser's own trace (on a touch screen a rule of #429 keyboard-hides-bar restyled every element of the
  page, about 5,000, at every show or hide).

The numbers are printed for each size. Pass --report-only to print them without failing, --screenshots DIR
to keep a PNG after the folds, and --web-root DIR to run against another copy of the page.
"""

from __future__ import annotations

import argparse
import os
import statistics
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import Json, check, handler_for, said
from two_column_browser import ColumnsApi, Desk, thread_record
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page

SIZES = (("phone-393", 393, 852, True), ("desk", 1280, 800, False))

TARGET = "spaces/A/threads/target"
OTHER = "spaces/A/threads/t18"
OTHER2 = "spaces/A/threads/t17"
PARA = ("The nightly run finished; the integration shard was retried twice and passed on the third attempt, "
        "and the artifact upload waited on the cache for most of an hour before it went through. ")


def long_reply(n: int) -> tuple[str, str]:
    """A reply as an agent writes one, and as src/render.rs renders it."""
    text = "\n\n".join([f"Reply {n}. " + PARA * 2, PARA, "```\ncargo test --release\nok 123 passed\n```", PARA])
    html = (f"<p>Reply {n}. {PARA * 2}</p>\n<p>{PARA}</p>\n<pre><code>cargo test --release\nok 123 passed\n"
            f"</code></pre>\n<p>{PARA}</p>\n")
    return text, html


class LatencyApi(ColumnsApi):
    """Twenty threads of long replies, Done and its Undo served."""

    def __init__(self) -> None:
        super().__init__()
        self.messages = []
        self.threads = []
        self.done: set[str] = set()
        index = 0
        for t in range(20):
            thread_id = TARGET if t == 19 else f"spaces/A/threads/t{t}"
            replies = 12 if thread_id == TARGET else 4
            root_at = index
            root = said(index, f"Thread {t} root. {PARA}", "coder")
            root["thread"] = thread_record(thread_id, index, replies)
            self.messages.append(root)
            index += 1
            last = root
            for r in range(replies):
                text, html = long_reply(r)
                row = said(index, text, "coder")
                row["content_html"] = html
                row["thread"] = thread_record(thread_id, root_at, replies, is_root=False)
                self.messages.append(row)
                last = row
                index += 1
            self.threads.append({"id": thread_id, "root": root, "title": "Thread", "reply_count": replies,
                                 "reply_count_exact": True, "updated_at": last["timestamp"]})

    def post(self, path: str, body: Json) -> tuple[int, Json]:
        ids = body.get("messages")
        named = [str(one) for one in ids] if isinstance(ids, list) else []
        if path.endswith("/dismiss"):
            with self.lock:
                self.done.update(named)
            return 200, {"dismissed": named}
        if path.endswith("/restore"):
            with self.lock:
                self.done.difference_update(named)
            return 200, {"messages": named}
        return super().post(path, body)

    def timeline(self, query: dict[str, list[str]]) -> Json:
        with self.lock:
            done = sorted(self.done)
        return {**super().timeline(query), "dismissed": done}


# Each key's handler, from the window's capture to the end of its dispatch (a window listener in the bubble
# phase, after the page's own on the document), and from the key to the frame after it; and the first frame
# each watched row was drawn folded or Done.
WATCH_JS = """(ids) => {
  window.__keys = [];
  window.__seen = {};
  window.__ids = ids;
  if (window.__watching) return;
  window.__watching = true;
  window.__longtasks = 0;
  try {
    new PerformanceObserver((list) => { window.__longtasks += list.getEntries().length; })
      .observe({type: 'longtask', buffered: true});
  } catch (error) { /* longtask timing unavailable in this browser */ }
  addEventListener('keydown', (event) => {
    if (event.key !== 'O' && event.key !== 'e') return;
    const press = {start: performance.now(), stamp: event.timeStamp, handler: null, frame: null};
    window.__keys.push(press);
    requestAnimationFrame(() => setTimeout(() => { press.frame = performance.now() - press.stamp; }, 0));
  }, true);
  addEventListener('keydown', (event) => {
    const press = window.__keys[window.__keys.length - 1];
    if (press && press.handler === null) press.handler = performance.now() - press.start;
  });
  const look = (now) => {
    for (const id of window.__ids) {
      if (window.__seen[id] !== undefined) continue;
      const li = document.querySelector(`#side-cards > li[data-unpack-thread][data-id='${id}']`);
      if (li && (li.getAttribute('data-read-run') || li.getAttribute('data-archived') === 'true')) window.__seen[id] = now;
    }
    requestAnimationFrame(look);
  };
  requestAnimationFrame(look);
}"""

BLOCK_IDS_JS = """(id) => [...document.querySelectorAll(`#side-cards > li[data-unpack-thread='${id}'][data-id]`)]
  .map((li) => li.getAttribute('data-id'))"""


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per size into (default: none)")
    parser.add_argument("--size", choices=[size[0] for size in SIZES], action="append",
                        help="walk only at these sizes, repeatable (default: all of them)")
    parser.add_argument("--cpu-ms", type=float, default=28.0,
                        help="most work a press's handler may take at the median, in ms of main-thread CPU time "
                             "corrected by the reference workload; twice it for the second largest press "
                             "(default: 28, ~1.5x the 19 ms solo median measured at 1280x800 on 2026-11-06)")
    parser.add_argument("--frame-ms", type=float, default=0.0,
                        help="most from a key to the next frame drawn at the median, in ms "
                             "(default: 0 = derived, 8x the work budget)")
    parser.add_argument("--budget-ms", type=float, default=None,
                        help="deprecated alias for --cpu-ms")
    parser.add_argument("--ref-idle-ms", type=float, default=4.4,
                        help="time of the reference layout workload in an uncontended browser of this host class, "
                             "in ms; each press's work is divided by how much slower the slower of the "
                             "reference runs bracketing that press ran (default: 4.4, measured 2026-11-06)")
    parser.add_argument("--restyle-max", type=int, default=500,
                        help="most elements hiding one row may restyle (default: 500)")
    parser.add_argument("--report-only", action="store_true",
                        help="print the numbers without failing on them (default: fail)")
    parser.add_argument("--benchmark", action="store_true",
                        help="also fail when the printed times exceed the work and frame budgets: an "
                             "opt-in speed measurement, never part of the gating verdict (default: "
                             "behaviour only)")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    if args.budget_ms is not None:
        args.cpu_ms = args.budget_ms
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required: python3 -m pip install --user playwright && "
                         "python3 -m playwright install chromium") from error
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, touch in SIZES:
            if args.size and label not in args.size:
                continue
            report.append(walk(playwright.chromium, args, label, width, height, touch))
    for line in report:
        print(line)
    return 0


def unpack_all(desk: Desk) -> None:
    for _ in range(3):
        if desk.page.get_attribute("#side-unpack", "aria-pressed") == "true":
            break
        desk.page.click("#side-unpack")
        desk.settled()
    desk.wait_for(f"() => document.querySelectorAll(\"#side-cards > li[data-unpack-thread='{TARGET}']\").length >= 12",
                  "the long thread did not unpack under its card", seconds=20)


def select(desk: Desk, row_id: str) -> None:
    desk.page.evaluate("(id) => { const li = document.querySelector(`#side-cards > li[data-unpack-thread][data-id='${id}']`);"
                       " li.scrollIntoView({block: 'center'}); selectRow(sidePane, li); }", row_id)
    desk.wait_for(f"() => document.querySelector(\"#side-cards > li[data-unpack-thread][data-id='{row_id}']\")?.getAttribute('data-selected') === 'true'",
                  f"the row {row_id} was not selected for the presses")
    desk.settled()


# A fixed document, the same whatever the page holds: sixty reply-shaped rows in a hidden iframe of the
# same renderer. Toggling a style that moves every row and reading their boxes is a few milliseconds of
# the style, layout and DOM work a fold does; how much longer it takes than --ref-idle-ms says how much
# slower this browser is running that kind of work right now (#998).
REF_SETUP_JS = """async () => {
  if (window.__refDoc) return;
  const frame = document.createElement('iframe');
  frame.setAttribute('aria-hidden', 'true');
  frame.style.cssText = 'position:fixed;left:-5000px;top:0;width:1200px;height:800px;border:0';
  const para = 'The nightly run finished; the integration shard was retried twice and passed. '.repeat(4);
  const rows = Array.from({length: 60}, (_, i) =>
    `<li class="row"><b>Reply ${i}</b><p>${para}</p><pre><code>cargo test\\nok ${i}</code></pre><p>${para}</p></li>`).join('');
  frame.srcdoc = `<!doctype html><style>.row{margin:4px;padding:6px;border:1px solid #444}
    body.alt .row{padding:7px;font-size:15px} p{line-height:1.4}</style><ul>${rows}</ul>`;
  await new Promise((resolve) => { frame.onload = resolve; document.body.appendChild(frame); });
  window.__refDoc = frame.contentDocument;
}"""

# Run in a timer task so the trace holds its main-thread CPU time (TimerFire thread time, matched by the
# timer's id): like the handlers, the reference must not count time the renderer was descheduled or
# throttled, or a throttled host would excuse a real regression (#998).
REF_JS = """() => new Promise((resolve) => { const id = setTimeout(() => {
  const doc = window.__refDoc; const t0 = performance.now();
  doc.body.classList.toggle('alt'); void doc.body.offsetHeight;
  for (const li of doc.querySelectorAll('li.row')) { if (li.getBoundingClientRect().height < 0) console.log(li); }
  resolve({id, ms: performance.now() - t0}); }, 0); })"""


def _psi_avg10(path: Path) -> tuple[str, str]:
    """(some, full) avg10 from a PSI pressure file; ("?", "?") when it cannot be read."""
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return "?", "?"
    found: dict[str, str] = {}
    for line in text.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[0] in ("some", "full"):
            for token in parts[1:]:
                if token.startswith("avg10="):
                    found[parts[0]] = token.removeprefix("avg10=")
    return found.get("some", "?"), found.get("full", "?")


def _load_parts() -> tuple[str, str, str, str, str]:
    """(gates running beside this one, loadavg 1/5/15, PSI memory some/full avg10, PSI CPU some
    avg10), each "?" when its source cannot be read."""
    admission = Path(os.environ.get("GATE_ADMISSION_DIR", "/tmp/dev-agent-utils-gate-admission")) / "runs"
    try:
        gates = str(sum(1 for _ in admission.glob("*.json")))
    except OSError:
        gates = "?"
    try:
        load = "/".join(Path("/proc/loadavg").read_text(encoding="utf-8").split()[:3])
    except OSError:
        load = "?"
    mem_some, mem_full = _psi_avg10(Path("/proc/pressure/memory"))
    cpu_some, _cpu_full = _psi_avg10(Path("/proc/pressure/cpu"))
    return gates, load, mem_some, mem_full, cpu_some


def load_evidence() -> str:
    """The host's load as this run found it, so a red run names the contention it ran under and a
    green one records what it survived (#1231)."""
    gates, load, mem_some, mem_full, cpu_some = _load_parts()
    return (f"host load: {gates} gate(s) running, loadavg {load}, "
            f"PSI memory some/full avg10 {mem_some}/{mem_full}, PSI CPU some avg10 {cpu_some}")


def load_snapshot() -> str:
    """One press's load, compact enough to print beside every press (#1231)."""
    gates, load, mem_some, mem_full, cpu_some = _load_parts()
    return f"gates {gates}, loadavg {load}, PSI mem {mem_some}/{mem_full}, PSI cpu {cpu_some}"


class Presses:
    """One key's presses: wall handler and key-to-frame times, when each watched row was drawn, the
    main thread's CPU time for each key's dispatch (None when the trace did not hold four), and the
    reference workload's time bracketing each press: `ref_before` just before it, `ref_after` just
    after it, and `ref` the slower of the two (its CPU time, or its wall time, said so, when the trace
    did not hold it)."""

    def __init__(self, handler: list[float], frame: list[float], seen: dict[str, float],
                 cpu: list[float] | None, ref: list[float], ref_before: list[float],
                 ref_after: list[float], loads: list[str], epoch: float = 0.0) -> None:
        self.handler, self.frame, self.seen, self.cpu, self.ref = handler, frame, seen, cpu, ref
        self.ref_before, self.ref_after = ref_before, ref_after
        # The host's load sampled at each press, printed beside it (#1231).
        self.loads = loads
        # When the first press started, on the page's own clock: a second batch's draw times are
        # rebased onto the first batch's by the difference of the two epochs.
        self.epoch = epoch
        self.ref_source = "CPU"


def timer_cpu_ms(events: list[Json], ids: list[int]) -> list[float] | None:
    """Thread CPU time of the timer tasks `ids`, in that order, from the trace's TimerFire events; None
    unless the trace holds each one."""
    cpu: dict[int, float] = {}
    for event in events:
        args = event.get("args")
        data = args.get("data") if isinstance(args, dict) else None
        tdur = event.get("tdur")
        if event.get("name") == "TimerFire" and isinstance(data, dict) and isinstance(tdur, (int, float)):
            timer = data.get("timerId")
            if isinstance(timer, int):
                cpu[timer] = float(tdur) / 1000.0
    return [cpu[id] for id in ids] if all(id in cpu for id in ids) else None


def key_cpu_ms(events: list[Json], count: int = 4) -> list[float] | None:
    """Thread CPU time of the keys' dispatches, in order, from the trace's keydown EventDispatch
    events. A Shift+O press also dispatches a keydown for Shift, which does next to nothing, so the
    `count` largest are the measured keys'."""
    dispatches = []
    for event in events:
        args = event.get("args")
        data = args.get("data") if isinstance(args, dict) else None
        cpu = event.get("tdur")
        if (event.get("name") == "EventDispatch" and isinstance(data, dict) and data.get("type") == "keydown"
                and isinstance(cpu, (int, float))):
            dispatches.append((float(str(event.get("ts") or 0)), float(cpu) / 1000.0))
    if len(dispatches) < count:
        return None
    largest = sorted(sorted(dispatches, key=lambda one: one[1])[-count:])
    return [cpu for _ts, cpu in largest]


def presses(desk: Desk, key: str, ids: list[str]) -> Presses:
    """One press of `key` 60 ms apart per watched row, watching `ids` for the frame each is drawn folded
    or Done, with the reference workload timed just before and just after each press and the browser's
    trace holding each dispatch's CPU time. Eight presses where the thread holds them: the median of
    eight is what the budget judges, and one contended press moves it far less than the median of
    four (#1231)."""
    page = desk.page
    count = len(ids)
    page.evaluate(REF_SETUP_JS)
    for _ in range(3):
        page.evaluate(REF_JS)  # warm: a new document's first layouts are not its steady cost
    page.evaluate(WATCH_JS, ids)
    cdp = page.context.new_cdp_session(page)
    events: list[Json] = []
    finished: list[bool] = []
    cdp.on("Tracing.dataCollected", lambda event: events.extend(event["value"]))
    cdp.on("Tracing.tracingComplete", lambda _event: finished.append(True))
    cdp.send("Tracing.start", {"transferMode": "ReportEvents",
                               "traceConfig": {"includedCategories": ["devtools.timeline"]}})
    ref_wall: list[float] = []
    ref_ids: list[int] = []
    loads: list[str] = []
    for _ in range(count):
        before = page.evaluate(REF_JS)
        assert isinstance(before, dict)
        ref_wall.append(float(str(before["ms"])))
        ref_ids.append(int(str(before["id"])))
        page.wait_for_timeout(30)
        page.keyboard.press(key)
        page.wait_for_timeout(60)
        after = page.evaluate(REF_JS)
        assert isinstance(after, dict)
        ref_wall.append(float(str(after["ms"])))
        ref_ids.append(int(str(after["id"])))
        loads.append(load_snapshot())
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline and not page.evaluate(
            f"() => window.__keys.length === {count} && window.__keys.every((press) => press.frame !== null)"):
        page.wait_for_timeout(50)
    cdp.send("Tracing.end")
    deadline = time.monotonic() + 10
    while not finished and time.monotonic() < deadline:
        page.wait_for_timeout(50)
    keys = page.evaluate("() => window.__keys")
    seen = page.evaluate("() => window.__seen")
    assert isinstance(keys, list) and isinstance(seen, dict)
    first = float(keys[0]["start"]) if keys else 0.0
    ref_cpu = timer_cpu_ms(events, ref_ids)
    bracketed = ref_cpu if ref_cpu is not None else ref_wall
    # Each press is judged against the slower of the reference runs bracketing it: contention that
    # lands on the press but had not yet landed on the run just before it is still counted (#1231).
    ref_before = bracketed[0::2]
    ref_after = bracketed[1::2]
    pressed = Presses([float(press["handler"] or 0) for press in keys], [float(press["frame"] or 0) for press in keys],
                      {str(id): float(at) - first for id, at in seen.items()}, key_cpu_ms(events, count),
                      [max(pair) for pair in zip(ref_before, ref_after)], ref_before, ref_after, loads, epoch=first)
    if ref_cpu is None:
        pressed.ref_source = "wall (no trace)"
    return pressed


def merged(first: Presses, second: Presses) -> Presses:
    """Two batches of one key's presses as one: the second batch's draw times rebased onto the first
    batch's first press, and CPU time kept only when both traces held every dispatch."""
    offset = second.epoch - first.epoch
    cpu = first.cpu + second.cpu if first.cpu is not None and second.cpu is not None else None
    both = Presses(first.handler + second.handler, first.frame + second.frame,
                   {**first.seen, **{row: at + offset for row, at in second.seen.items()}}, cpu,
                   first.ref + second.ref, first.ref_before + second.ref_before,
                   first.ref_after + second.ref_after, first.loads + second.loads, epoch=first.epoch)
    both.ref_source = first.ref_source if first.ref_source == second.ref_source else "mixed"
    return both


def judge(desk: Desk, args: argparse.Namespace, what: str, pressed: Presses, ids: list[str]) -> tuple[bool, str]:
    """Whether these presses did what the key promises, and the one line that says so: each press's
    work, wall and reference times, the rows' draw times, and the host's load as the run found it.
    The verdict is behaviour only — every press heard, and every watched row drawn folded or Done.
    The times are printed for every run but decide nothing in gating; `--benchmark` enforces the
    budgets below as an opt-in measurement outside the gate. The caller decides what a failure
    means; this only measures."""
    cpu_ms = args.cpu_ms
    frame_ms = args.frame_ms if args.frame_ms > 0 else 8 * cpu_ms
    handler, frame, seen = pressed.handler, pressed.frame, pressed.seen
    factors = [max(1.0, ms / args.ref_idle_ms) for ms in pressed.ref]
    # The main thread's CPU time when the trace held every key; else the wall handler times, which a
    # loaded host stretches, and the report says so.
    if pressed.cpu is not None and len(pressed.cpu) == len(handler):
        measured, source = pressed.cpu, "CPU"
    else:
        measured, source = handler, "wall (no trace)"
    work = [ms / factor for ms, factor in zip(measured, factors)]
    # The per-press ceiling is not the largest press (#905): the largest quarter of presses can still
    # meet a stall the reference bracketing them missed, while sustained main-thread work, the
    # regression this budget exists to catch, lifts every press.
    spikes = max(1, len(work) // 4)
    trimmed_max = sorted(work)[-1 - spikes] if len(work) > spikes else (work[0] if work else 0.0)
    longtasks = desk.page.evaluate("() => window.__longtasks || 0")
    per_press = "; ".join(
        f"press {index + 1}: work {one_work:.0f} ms, wall {one_wall:.0f} ms, "
        f"ref {one_ref:.1f} ms before/{pressed.ref_after[index]:.1f} ms after (x{one_factor:.2f}) "
        f"[{pressed.loads[index] if index < len(pressed.loads) else 'load ?'}]"
        for index, (one_work, one_wall, one_ref, one_factor) in enumerate(zip(work, handler, pressed.ref_before, factors)))
    said = (f"{what}: handlers {', '.join(f'{ms:.0f}' for ms in handler)} ms wall, "
            f"{', '.join(f'{ms:.0f}' for ms in measured)} ms {source}, "
            f"{', '.join(f'{ms:.0f}' for ms in work)} ms work at load {', '.join(f'x{f:.2f}' for f in factors)}"
            f" ({pressed.ref_source} reference), "
            f"key to frame {', '.join(f'{ms:.0f}' for ms in frame)} ms, "
            f"rows drawn at {', '.join(f'{seen.get(id, -1):.0f}' for id in ids)} ms"
            f" (work budget {cpu_ms:.0f} ms, wall budget {frame_ms:.0f} ms, {longtasks} long tasks); "
            f"{per_press}; {load_evidence()}")
    print(said, flush=True)
    if args.report_only:
        return True, said
    heard = len(handler) == len(ids)
    drawn = all(id in seen for id in ids)
    if not heard:
        return False, f"{what}: not every press heard: {said}"
    if not drawn:
        return False, f"{what}: not every press drew its row: {said}"
    if args.benchmark:
        within = bool(work) and statistics.median(work) <= cpu_ms and trimmed_max <= 2 * cpu_ms
        framed = bool(frame) and statistics.median(frame) <= frame_ms
        if not within:
            return False, f"{what}: over the {cpu_ms:.0f} ms work budget: {said}"
        if not framed:
            return False, f"{what}: the frames came late: {said}"
    return True, said


RESTYLE_JS = """() => { const rows = [...document.querySelectorAll('#side-cards > li[data-unpack-thread][data-id]')];
  window.__hide = rows[2]; document.body.offsetHeight; }"""


def restyled(page: Page) -> int:
    """How many elements the browser restyles when one unpacked row is hidden, from its own trace."""
    page.evaluate(RESTYLE_JS)
    cdp = page.context.new_cdp_session(page)
    events: list[Json] = []
    finished: list[bool] = []
    cdp.on("Tracing.dataCollected", lambda event: events.extend(event["value"]))
    cdp.on("Tracing.tracingComplete", lambda _event: finished.append(True))
    cdp.send("Tracing.start", {"transferMode": "ReportEvents", "traceConfig": {
        "includedCategories": ["devtools.timeline", "disabled-by-default-devtools.timeline"]}})
    page.evaluate("() => { window.__hide.hidden = true; document.body.offsetHeight; window.__hide.hidden = false; }")
    cdp.send("Tracing.end")
    deadline = time.monotonic() + 10
    while not finished and time.monotonic() < deadline:
        page.wait_for_timeout(50)
    counts = []
    for event in events:
        data = event.get("args")
        begin = data.get("beginData") if isinstance(data, dict) else None
        if event.get("name") == "UpdateLayoutTree" and isinstance(data, dict):
            counts.append(int(str(data.get("elementCount") or (begin or {}).get("elementCount") or 0)))
    return max(counts, default=0)


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, touch: bool) -> str:
    api = LatencyApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-fold-latency-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark", has_touch=touch, is_mobile=touch)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"fold-latency-{label}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            page.fill("#api-token", fixture.TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            page.click("#view-switch")
            desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length >= 5",
                          "the channel did not open", seconds=20)
            page.select_option("#thread-select", "threads")
            desk.wait_for("() => document.querySelectorAll('#side-cards > li[data-card]').length >= 20",
                          "the Threads view has not every card", seconds=20)
            unpack_all(desk)
            page.wait_for_timeout(1000)
            rows = int(page.evaluate("() => document.querySelectorAll('#side-cards > li').length"))
            notes = [f"{rows} rows in the Threads view"]
            # A first key, away from what is measured: the page's first key in a session says the keys are
            # in use on the root, which styles the page once.
            page.keyboard.press("ArrowUp")
            desk.settled()

            target = page.evaluate(BLOCK_IDS_JS, TARGET)
            assert isinstance(target, list)
            select(desk, str(target[-1]))
            folded = [str(one) for one in target[-8:]][::-1]
            within, said = judge(desk, args, "Shift+O", presses(desk, "Shift+O", folded), folded)
            notes.append(said)
            desk.check(within, said)
            desk.check(page.evaluate("(id) => document.querySelector(`#side-cards > li[data-unpack-thread][data-id='${id}']`)"
                                     ".getAttribute('data-read-run')", folded[-1]) == "head",
                       "the eight folds are not one line")
            desk.shot("1-folded")

            other = page.evaluate(BLOCK_IDS_JS, OTHER)
            assert isinstance(other, list)
            select(desk, str(other[0]))
            marked = [str(one) for one in other[:4]]
            # `#1321 after-done-setting`: e stepping down the rows is the Next unread choice.
            page.evaluate("() => localStorage.setItem('vibe-talk.voice.after-done', 'unread')")
            first_batch = presses(desk, "e", marked)
            other2 = page.evaluate(BLOCK_IDS_JS, OTHER2)
            assert isinstance(other2, list)
            select(desk, str(other2[0]))
            marked2 = [str(one) for one in other2[:4]]
            marked += marked2
            within, said = judge(desk, args, "e", merged(first_batch, presses(desk, "e", marked2)), marked)
            notes.append(said)
            desk.check(within, said)
            desk.shot("2-done")

            count = restyled(page)
            notes.append(f"hiding one row restyled {count} elements")
            if not args.report_only:
                desk.check(count <= args.restyle_max, f"hiding one row restyled {count} elements, over {args.restyle_max}")
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}: " + "; ".join(notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
