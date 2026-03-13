"""Tests for SQLite generation from CSVs in db_generator.py."""
from __future__ import annotations

import sqlite3
from pathlib import Path

import pytest

from app.db_generator import generate_sqlite


@pytest.fixture()
def db_dir(tmp_path: Path) -> Path:
    """Create a minimal database directory with two category CSVs."""
    d = tmp_path / "database"
    d.mkdir()
    (d / "g-res.csv").write_text(
        "IPN,Description,Value\n"
        "RES-0001-1004,10k Resistor,10000\n"
        "RES-0002-1003,100 Resistor,100\n",
        encoding="utf-8",
    )
    (d / "g-cap.csv").write_text(
        "IPN,Description,Value\n"
        "CAP-0001-0104,100nF Cap,100n\n",
        encoding="utf-8",
    )
    return d


class TestGenerateSqlite:
    def test_creates_db_file(self, db_dir: Path, tmp_path: Path):
        out = tmp_path / "parts.sqlite"
        generate_sqlite(db_dir, out)
        assert out.exists()

    def test_has_tables(self, db_dir: Path, tmp_path: Path):
        out = tmp_path / "parts.sqlite"
        generate_sqlite(db_dir, out)
        conn = sqlite3.connect(out)
        tables = {r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall()}
        conn.close()
        assert "res" in tables
        assert "cap" in tables

    def test_row_count(self, db_dir: Path, tmp_path: Path):
        out = tmp_path / "parts.sqlite"
        generate_sqlite(db_dir, out)
        conn = sqlite3.connect(out)
        count = conn.execute("SELECT COUNT(*) FROM res").fetchone()[0]
        conn.close()
        assert count == 2

    def test_column_values(self, db_dir: Path, tmp_path: Path):
        out = tmp_path / "parts.sqlite"
        generate_sqlite(db_dir, out)
        conn = sqlite3.connect(out)
        row = conn.execute("SELECT IPN, Description FROM cap").fetchone()
        conn.close()
        assert row[0] == "CAP-0001-0104"
        assert "100nF" in row[1]

    def test_progress_callback(self, db_dir: Path, tmp_path: Path):
        out = tmp_path / "parts.sqlite"
        calls: list[tuple] = []
        generate_sqlite(db_dir, out, progress_cb=lambda i, t, n: calls.append((i, t, n)))
        assert len(calls) == 2
        assert calls[-1][0] == 2  # second file processed

    def test_empty_dir(self, tmp_path: Path):
        d = tmp_path / "empty_db"
        d.mkdir()
        out = tmp_path / "empty.sqlite"
        generate_sqlite(d, out)
        assert out.exists()

    def test_null_values_become_empty_string(self, tmp_path: Path):
        d = tmp_path / "db"
        d.mkdir()
        (d / "g-tst.csv").write_text("IPN,Description,Extra\nTST-0001-0001,test\n", encoding="utf-8")
        out = tmp_path / "test.sqlite"
        generate_sqlite(d, out)
        conn = sqlite3.connect(out)
        row = conn.execute("SELECT Extra FROM tst").fetchone()
        conn.close()
        assert row[0] == ""
