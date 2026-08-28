"""Source-checkout test configuration for the wrkslots distribution."""

from __future__ import annotations

import sys
from pathlib import Path


PY_ROOT = Path(__file__).resolve().parents[2]
if str(PY_ROOT) not in sys.path:
    sys.path.insert(0, str(PY_ROOT))
