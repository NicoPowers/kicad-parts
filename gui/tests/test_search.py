"""Tests for search_rows in search.py."""
from __future__ import annotations

import pytest

from app.search import SearchHit, search_rows


SAMPLE_ROWS = [
    {"IPN": "RES-0001-1004", "MPN": "RC0603FR-0710KL", "Description": "RES 10k 1% 0603"},
    {"IPN": "RES-0002-1003", "MPN": "ERJ-3GEYJ101V", "Description": "RES 100 5% 0603"},
    {"IPN": "CAP-0001-0104", "MPN": "GRM188R71H104KA93", "Description": "CAP 100nF 10% 0603"},
]


class TestSearchRows:
    def test_match_by_ipn(self):
        hits = search_rows("res", SAMPLE_ROWS, "RES-0001")
        assert len(hits) == 1
        assert hits[0].ipn == "RES-0001-1004"

    def test_match_by_mpn(self):
        hits = search_rows("res", SAMPLE_ROWS, "RC0603")
        assert len(hits) == 1

    def test_match_by_description(self):
        hits = search_rows("mixed", SAMPLE_ROWS, "100nF")
        assert len(hits) == 1
        assert hits[0].ipn == "CAP-0001-0104"

    def test_case_insensitive(self):
        hits = search_rows("res", SAMPLE_ROWS, "res 10k")
        assert len(hits) == 1

    def test_no_match(self):
        hits = search_rows("res", SAMPLE_ROWS, "xyz_nothing")
        assert hits == []

    def test_empty_query(self):
        assert search_rows("res", SAMPLE_ROWS, "") == []

    def test_whitespace_query(self):
        assert search_rows("res", SAMPLE_ROWS, "   ") == []

    def test_broad_match(self):
        hits = search_rows("res", SAMPLE_ROWS, "0603")
        assert len(hits) == 3

    def test_result_fields(self):
        hits = search_rows("cat", SAMPLE_ROWS, "RES-0001")
        assert isinstance(hits[0], SearchHit)
        assert hits[0].category == "cat"
        assert hits[0].description == "RES 10k 1% 0603"
