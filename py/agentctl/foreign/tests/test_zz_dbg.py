from __future__ import annotations
import json, os, signal, sys, time, subprocess
from pathlib import Path
import pytest
from agentctl.foreign import lib
from agentctl.foreign.tests.test_agent_runner import (_install_agy_agent, _runner_process, _wait_for,
    _kill_started, fake_runner_state)  # noqa: F401

@pytest.mark.parametrize("i", range(40))
@pytest.mark.parametrize("harness", ["codex", "agy"])
def test_signal(fake_runner_state: Path, harness: str, i: int) -> None:
    rec = _install_agy_agent(fake_runner_state, "busy")
    with lib.registry_lock() as agents:
        agents[rec.name].harness = harness
    executable = fake_runner_state / "sleeping-harness"
    executable.write_text(f"#!{sys.executable}\n" +
        "import json, os, subprocess, sys, time\nfrom pathlib import Path\n"
        "if 'exec' in sys.argv: sys.stdin.read()\n"
        "child=subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])\n"
        "temporary=Path(sys.argv[0]+'.ready.tmp')\n"
        "temporary.write_text(json.dumps([os.getpid(),child.pid]))\n"
        "os.replace(temporary,Path(sys.argv[0]+'.ready'))\n"
        "time.sleep(60)\n")
    executable.chmod(0o755)
    lib.enqueue_message(rec.name, "bounded owned-process test", model=None)
    runner = _runner_process(fake_runner_state, rec.name, executable)
    started = {}
    try:
        ready = Path(str(executable) + ".ready")
        _wait_for(ready, runner)
        pids = json.loads(ready.read_text())
        started = {pid: lib.pid_start_time(pid) for pid in pids}
        _wait_for(lib.agent_dir(rec.name) / "active-harness.json", runner)
        runner.terminate()
        try:
            runner.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            os.kill(runner.pid, signal.SIGUSR1)
            time.sleep(1)
            runner.kill()
            out, err = runner.communicate(timeout=5)
            states = {pid: Path(f"/proc/{pid}/stat").read_text() if Path(f"/proc/{pid}").exists() else "gone" for pid in pids}
            pytest.fail(f"HANG states={states}\nstderr={err[-6000:]}\ntranscript={lib.transcript_path(rec.name).read_text()[-1000:]}")
    finally:
        if runner.poll() is None:
            runner.kill()
            runner.communicate(timeout=5)
        for pid, st in started.items():
            _kill_started(pid, st)
