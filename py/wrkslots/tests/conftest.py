"""Source-checkout test configuration for the wrkslots distribution."""

from __future__ import annotations

import sys
from pathlib import Path


PY_ROOT = Path(__file__).resolve().parents[2]
if str(PY_ROOT) not in sys.path:
    sys.path.insert(0, str(PY_ROOT))

# The suite predates disk-image slots and exercises the plain-worktree
# representation; image slots have their own tests (test_slot_images.py and
# e2e_images.sh). New projects created by these tests therefore default to
# worktrees. Subprocesses inherit this.
import os  # noqa: E402

os.environ.setdefault("WRKSLOTS_INIT_REPRESENTATION", "worktree")
