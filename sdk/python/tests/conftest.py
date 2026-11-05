"""Make ``src/`` importable for the tests (src-layout, no install required).

pytest auto-loads this. It lets the suite run against the working tree exactly
as it would run against an installed package; the transport is still real HTTP.
"""

from __future__ import annotations

import os
import sys

_SRC = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src")
if _SRC not in sys.path:
    sys.path.insert(0, _SRC)
