"""Tests for the SubstitutesStore in substitutes.py."""
from __future__ import annotations

from pathlib import Path

import pytest

from app.substitutes import SubstituteRecord, SubstitutesStore


@pytest.fixture()
def store_path(tmp_path: Path) -> Path:
    return tmp_path / "substitutes.csv"


class TestSubstitutesStore:
    def test_load_missing_file(self, store_path: Path):
        store = SubstitutesStore(store_path)
        assert store.records == []

    def test_add_and_save(self, store_path: Path):
        store = SubstitutesStore(store_path)
        store.add(SubstituteRecord("RES-0001-1004", "RC0603FR-0710KL", "Yageo", "", "DigiKey", "311-10.0KHRCT-ND"))
        assert store_path.exists()
        assert len(store.records) == 1

    def test_reload(self, store_path: Path):
        store = SubstitutesStore(store_path)
        store.add(SubstituteRecord("RES-0001-1004", "MPN-A", "Mfr-A", "", "DigiKey", "DK-A"))
        store.add(SubstituteRecord("RES-0001-1004", "MPN-B", "Mfr-B", "", "Mouser", "MS-B"))
        store.add(SubstituteRecord("CAP-0001-0104", "MPN-C", "Mfr-C", "", "LCSC", "LC-C"))

        reloaded = SubstitutesStore(store_path)
        assert len(reloaded.records) == 3

    def test_by_ipn(self, store_path: Path):
        store = SubstitutesStore(store_path)
        store.add(SubstituteRecord("RES-0001-1004", "MPN-A", "Mfr-A", "", "DigiKey", "DK-A"))
        store.add(SubstituteRecord("CAP-0001-0104", "MPN-B", "Mfr-B", "", "Mouser", "MS-B"))

        results = store.by_ipn("RES-0001-1004")
        assert len(results) == 1
        assert results[0].mpn == "MPN-A"

    def test_by_ipn_empty(self, store_path: Path):
        store = SubstitutesStore(store_path)
        assert store.by_ipn("NONEXISTENT") == []

    def test_record_fields(self, store_path: Path):
        store = SubstitutesStore(store_path)
        store.add(SubstituteRecord("IND-0001-0001", "SRR1005-100M", "Bourns", "https://ds.pdf", "Mouser", "652-SRR"))

        reloaded = SubstitutesStore(store_path)
        r = reloaded.records[0]
        assert r.ipn == "IND-0001-0001"
        assert r.mpn == "SRR1005-100M"
        assert r.manufacturer == "Bourns"
        assert r.datasheet == "https://ds.pdf"
        assert r.supplier == "Mouser"
        assert r.supplier_pn == "652-SRR"
