"""Tests for CSV reading, writing, and sorting in csv_manager.py."""
from __future__ import annotations

from pathlib import Path

import pytest

from app.csv_manager import CsvDocument, read_csv, sort_rows_by_ipn, write_csv


@pytest.fixture()
def sample_csv(tmp_path: Path) -> Path:
    p = tmp_path / "sample.csv"
    p.write_text(
        "IPN,Description,Value\n"
        "RES-0002-1004,10k Resistor,10000\n"
        "RES-0001-1003,100 Resistor,100\n",
        encoding="utf-8",
    )
    return p


@pytest.fixture()
def quoted_csv(tmp_path: Path) -> Path:
    p = tmp_path / "quoted.csv"
    p.write_text(
        '"IPN","Description","Value"\n'
        '"RES-0001-1004","10k Resistor","10000"\n',
        encoding="utf-8",
    )
    return p


class TestReadCsv:
    def test_reads_headers(self, sample_csv: Path):
        doc = read_csv(sample_csv)
        assert doc.headers == ["IPN", "Description", "Value"]

    def test_reads_rows(self, sample_csv: Path):
        doc = read_csv(sample_csv)
        assert len(doc.rows) == 2

    def test_row_values(self, sample_csv: Path):
        doc = read_csv(sample_csv)
        assert doc.rows[0]["IPN"] == "RES-0002-1004"

    def test_null_values_become_empty(self, tmp_path: Path):
        """If a CSV row has fewer columns than the header, missing fields are ''."""
        p = tmp_path / "short.csv"
        p.write_text("IPN,Description,Value\nRES-0001-1004\n", encoding="utf-8")
        doc = read_csv(p)
        assert doc.rows[0]["Description"] == ""
        assert doc.rows[0]["Value"] == ""

    def test_detect_quote_all(self, quoted_csv: Path):
        doc = read_csv(quoted_csv)
        assert doc.quote_all is True

    def test_detect_no_quote(self, sample_csv: Path):
        doc = read_csv(sample_csv)
        assert doc.quote_all is False


class TestWriteCsv:
    def test_round_trip(self, sample_csv: Path):
        doc = read_csv(sample_csv)
        doc.rows.append({"IPN": "RES-0003-1005", "Description": "100k Resistor", "Value": "100000"})
        write_csv(doc, make_backup=False)
        reloaded = read_csv(sample_csv)
        assert len(reloaded.rows) == 3

    def test_backup_created(self, sample_csv: Path):
        doc = read_csv(sample_csv)
        write_csv(doc, make_backup=True)
        assert sample_csv.with_suffix(".csv.bak").exists()

    def test_sorted_by_ipn(self, sample_csv: Path):
        doc = read_csv(sample_csv)
        write_csv(doc, make_backup=False)
        reloaded = read_csv(sample_csv)
        ipns = [r["IPN"] for r in reloaded.rows]
        assert ipns == sorted(ipns)

    def test_quote_all_preserved(self, quoted_csv: Path):
        doc = read_csv(quoted_csv)
        write_csv(doc, make_backup=False)
        content = quoted_csv.read_text(encoding="utf-8")
        assert content.startswith('"IPN"')


class TestSortRowsByIPN:
    def test_sorts_ascending(self):
        rows = [{"IPN": "B"}, {"IPN": "A"}, {"IPN": "C"}]
        result = sort_rows_by_ipn(rows)
        assert [r["IPN"] for r in result] == ["A", "B", "C"]

    def test_empty(self):
        assert sort_rows_by_ipn([]) == []
