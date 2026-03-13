"""Tests for search_rows in search.py."""
from __future__ import annotations

import pytest

from app.search import (
    LocalSearchSummary,
    SearchHit,
    search_components,
    search_local_inventory,
    search_rows,
    should_search_remote,
)


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


class TestUnifiedSearch:
    def test_local_inventory_ranks_exact_match_high(self):
        categories = {"res": SAMPLE_ROWS}
        summary = search_local_inventory(categories, "RC0603FR-0710KL")
        assert summary.hits
        assert summary.hits[0].mpn == "RC0603FR-0710KL"
        assert summary.confidence in {"high", "medium"}
        assert summary.best_score > 0.7

    def test_local_inventory_value_equivalence(self):
        categories = {
            "res": [
                {
                    "IPN": "RES-0001-1004",
                    "MPN": "RC0603FR-0710KL",
                    "Description": "RES 10k 1% 0603",
                    "Value": "10k",
                    "Manufacturer": "Yageo",
                }
            ]
        }
        summary = search_local_inventory(categories, "10000")
        assert summary.hits
        assert summary.hits[0].ipn == "RES-0001-1004"
        assert summary.hits[0].score >= 0.8

    def test_should_search_remote_for_low_or_none(self):
        assert should_search_remote(LocalSearchSummary(query="x", hits=[], confidence="none", best_score=0.0))
        assert should_search_remote(LocalSearchSummary(query="x", hits=[], confidence="low", best_score=0.49))
        assert not should_search_remote(LocalSearchSummary(query="x", hits=[], confidence="high", best_score=0.9))
        assert should_search_remote(
            LocalSearchSummary(query="x", hits=[], confidence="high", best_score=0.9),
            force_remote=True,
        )

    def test_search_components_auto_fallback(self):
        categories = {"res": SAMPLE_ROWS}
        calls: list[tuple[str, int]] = []

        def fake_remote(query: str, limit: int) -> list[str]:
            calls.append((query, limit))
            return ["remote-1"]

        result = search_components("not-likely-present", categories, fake_remote, remote_limit=3)
        assert result.remote_requested
        assert result.remote_results == ["remote-1"]
        assert calls == [("not-likely-present", 3)]

    def test_search_components_skips_remote_on_confident_local(self):
        categories = {"res": SAMPLE_ROWS}
        calls: list[tuple[str, int]] = []

        def fake_remote(query: str, limit: int) -> list[str]:
            calls.append((query, limit))
            return ["remote-1"]

        result = search_components("RES-0001-1004", categories, fake_remote)
        assert not result.remote_requested
        assert result.remote_results == []
        assert calls == []

    def test_search_components_force_remote(self):
        categories = {"res": SAMPLE_ROWS}
        calls: list[tuple[str, int]] = []

        def fake_remote(query: str, limit: int) -> list[str]:
            calls.append((query, limit))
            return ["remote-1", "remote-2"]

        result = search_components("RES-0001-1004", categories, fake_remote, force_remote=True, remote_limit=2)
        assert result.remote_requested
        assert result.remote_results == ["remote-1", "remote-2"]
        assert calls == [("RES-0001-1004", 2)]
