"""Shared fixtures for the GUI test suite."""
from __future__ import annotations

import sys
from pathlib import Path

import pytest

# Ensure gui/app is importable as `app.*` when running from gui/.
GUI_DIR = Path(__file__).resolve().parents[1]
if str(GUI_DIR) not in sys.path:
    sys.path.insert(0, str(GUI_DIR))


def pytest_configure(config: pytest.Config) -> None:
    config.addinivalue_line("markers", "live: hit real supplier APIs (requires secrets.env)")
