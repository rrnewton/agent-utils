# `examples/` — runnable safe-ci-dag-runner DAGs

Five small, self-contained DAG files you can run immediately, each demonstrating one core idea.
Every command is `sleep`/`echo` only, so a whole example finishes in a few seconds and needs no build
tools installed. Each example also carries `description` fields (a top-level one for the whole DAG
and one per node) — free-form documentation that never affects scheduling.

Two of them (`02-diamond` and `04-memory-aware`) additionally ship a **side-by-side YAML edition**
(`.yaml`) that loads to the *exact same DAG* as its `.json` twin — a "literate" version with inline
comments and multi-line block-scalar descriptions. `--dag` auto-detects the format by extension
(`.yaml`/`.yml` → YAML, else JSON), so you can point `run`/`list`/`ascii`/`dot`/`json`/`yaml` at
either file. See `02-diamond.yaml` for what a literate DAG looks like.

Run any of them with either build (they behave identically — that parity is enforced by
`cross/differential.py`):

```sh
# Python (no build needed):
python3 -m safe_ci_dag_runner run --dag examples/01-linear-chain.json --allow-cgroup-failure
# or, once installed / built:
safe-ci-dag-runner run --dag examples/01-linear-chain.json --allow-cgroup-failure
```

**Why `--allow-cgroup-failure`?** Boxing each step in its own Linux cgroup-v2 sandbox is this
tool's whole point, so `run` **boxes by default**: it re-execs the run inside a transient
`systemd-run --user --scope` and caps each step. On a machine without cgroup-v2 + a working systemd
`--user` scope (many laptops, most CI runners, macOS), a bare `run` would instead **error with exit
code 3** rather than silently run unprotected. `--allow-cgroup-failure` opts out of the requirement:
it runs the steps **un-boxed** (with a visible warning) so these demo DAGs work out-of-the-box
anywhere. Drop the flag on a Linux host with a systemd user session to get the real per-step boxing.
The commands below include it so they are copy-paste runnable on any machine.

Before running, it is worth *looking* at each graph — every example works with `list`, `ascii`, `dot`,
`json`, and `yaml` too:

```sh
safe-ci-dag-runner list  --dag examples/02-diamond.json   # one line per step, with class + deps
safe-ci-dag-runner ascii --dag examples/02-diamond.json   # topological-layer view
safe-ci-dag-runner dot   --dag examples/02-diamond.json | dot -Tsvg -o dag.svg   # a picture
```

Work through them in order — each adds one concept on top of the previous.

## 1. `01-linear-chain.json` — a trivial linear chain

`build.compile → test.unit → package.tarball`: the simplest possible pipeline, three steps in a
straight line where each waits for the previous one. Start here to see the basic run output (a
`▶ START` / `✓ PASS` line per step and the final `PASS` summary). Because each step depends on the one
before it, nothing runs in parallel and the wall time is the sum of the steps (~6s).

```sh
safe-ci-dag-runner run --dag examples/01-linear-chain.json --allow-cgroup-failure
```

## 2. `02-diamond.json` — a diamond with parallel branches

`build.app` fans out to two independent checks (`test.unit` and `test.lint`) that run **at the same
time**, and `deploy.staging` waits for **both** to finish. This is the first example where you see
concurrency: the two middle steps overlap, so the wall time is shorter than their sum. `test.unit`
carries a larger `est_duration_s`, so when both become ready the runner dispatches it first
(longest-processing-time ordering). If either check failed, `deploy.staging` would be reported as
`skipped`.

```sh
safe-ci-dag-runner run --dag examples/02-diamond.json --allow-cgroup-failure
safe-ci-dag-runner ascii --dag examples/02-diamond.json   # shows the 3 topological layers
```

This example also ships as **`02-diamond.yaml`** — the same DAG, written literately: inline `#`
comments and multi-line block-scalar (`|-` / `>-`) descriptions. It loads identically (proven by
`cross/differential.py`), so you can run the YAML edition anywhere the JSON one runs:

```sh
safe-ci-dag-runner run  --dag examples/02-diamond.yaml --allow-cgroup-failure
safe-ci-dag-runner json --dag examples/02-diamond.yaml   # YAML in, canonical JSON out
```

## 3. `03-scarce-resource-browser.json` — a named scarce resource

Two end-to-end steps (`e2e.login`, `e2e.checkout`) each declare `"resources": {"browser": 1}`, and the
top-level `"resource_caps": {"browser": 1}` allows only **one** browser step to run at a time. So the
two e2e steps run one-after-another even though both are ready — while the ordinary `test.unit` step
(which needs no browser) runs alongside whichever e2e step currently holds the browser.

`browser` here is just a **name you chose** — the caps are arbitrary caller-defined strings, not a
built-in. It models any scarce thing that cannot be shared concurrently (in DeepScry it serializes
Playwright/browser end-to-end tests, which grab fixed ports and a display). You could equally cap
`"gpu"`, `"db"`, or `"licenses"`.

```sh
safe-ci-dag-runner run --dag examples/03-scarce-resource-browser.json --allow-cgroup-failure
```

## 4. `04-memory-aware.json` — memory hints drive a RAM-safe `-j`

Each step carries a memory hint: `build.compile` and `test.integration` set `rss_baseline_bytes`
(estimated peak RSS), and `test.fuzz` additionally pins a `hard_mem_max_bytes` cap that overrides its
baseline. With those hints, `--max-mem` picks the largest `-j` whose modeled worst-case footprint fits
a RAM budget, instead of you hard-coding a number:

```sh
safe-ci-dag-runner run --dag examples/04-memory-aware.json --max-mem 4G --allow-cgroup-failure   # -> -j1 (worst case 4 GiB)
safe-ci-dag-runner run --dag examples/04-memory-aware.json --max-mem 8G --allow-cgroup-failure   # -> -j2 (worst case 7 GiB)
```

A 4 GiB budget only fits one step at a time; 8 GiB fits the two largest that can co-run; a large enough
budget lets all three run at once (up to the CPU count). Run it with no `--max-mem` and it simply uses
the CPU count as `-j`. (This example sets `mem_cap_factor: 1.0`, `mem_cap_floor_bytes: 0`, and
`outer_mem_safety_factor: 1.0` so the modeled numbers are exactly the byte values above; the defaults
add headroom.)

This example also ships as **`04-memory-aware.yaml`** — the same DAG in literate YAML (byte-value
comments on each memory hint), loading identically to the JSON:

```sh
safe-ci-dag-runner run --dag examples/04-memory-aware.yaml --max-mem 8G --allow-cgroup-failure
```

## 5. `05-inner-jobs.json` — a step with internal parallelism

`build.app` runs its *own* parallel build (imagine `make -j8`). Declaring
`"preferred_inner_jobs": 8` tells the runner how wide the step is internally, so it can set an inner
CPU cap when cgroup boxing is active and account for the extra memory a wide `cpu-bound` step uses in
the budget model.

By **default** the runner also *appends* an inner-jobs flag to your command derived from that width —
the default template is `-j`, so a `cmd` of `make build` would be run as `make build -j 8`. This step,
though, already hardcodes its own `make -j8`, so it sets `"jobs_flag": ""` to **opt out** of the
append (otherwise the runner would tack a redundant `-j 8` onto the command — and appending it to this
example's `sleep`-based simulated command would even make it error). Set `jobs_flag` to a template
like `"-j%d"`, `"--jobs="`, or `"--num-threads"` to control the flag's spelling, or `""` (as here)
when your command manages its own concurrency.

```sh
safe-ci-dag-runner run --dag examples/05-inner-jobs.json --allow-cgroup-failure
```

## See also

- `common/docs/safe-ci-dag-runner/README.md` — the tool overview and CLI reference.
- `common/docs/safe-ci-dag-runner/USER_GUIDE.md` — the full concept guide, the complete JSON/YAML
  schema, and the YAML isomorphism details.
- `safe-ci-dag-runner quickstart` — the same getting-started tour from the command line.
