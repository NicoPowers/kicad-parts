"""Tests for cell validation and duplicate detection in validators.py."""
from __future__ import annotations

import pytest

from app.validators import find_duplicate_values, is_valid_url, validate_cell


# ---------------------------------------------------------------------------
# is_valid_url
# ---------------------------------------------------------------------------

class TestIsValidUrl:
    def test_empty_is_valid(self):
        assert is_valid_url("") is True

    def test_https(self):
        assert is_valid_url("https://example.com/datasheet.pdf") is True

    def test_http(self):
        assert is_valid_url("http://example.com") is True

    def test_no_scheme(self):
        assert is_valid_url("example.com") is False

    def test_ftp_rejected(self):
        assert is_valid_url("ftp://files.example.com") is False

    def test_garbage(self):
        assert is_valid_url("not a url at all") is False


# ---------------------------------------------------------------------------
# validate_cell
# ---------------------------------------------------------------------------

class TestValidateCell:
    def test_valid_ipn(self):
        assert validate_cell("IPN", "RES-0001-1004") is None

    def test_invalid_ipn(self):
        err = validate_cell("IPN", "bad")
        assert err is not None
        assert "CCC-NNNN-VVVV" in err

    def test_valid_datasheet_url(self):
        assert validate_cell("Datasheet", "https://example.com") is None

    def test_invalid_datasheet_url(self):
        err = validate_cell("Datasheet", "not-a-url")
        assert err is not None

    def test_empty_datasheet_ok(self):
        assert validate_cell("Datasheet", "") is None

    def test_required_empty(self):
        err = validate_cell("Description", "")
        assert err is not None
        assert "required" in err.lower()

    def test_required_filled(self):
        assert validate_cell("Description", "10k Resistor") is None

    def test_non_required_empty(self):
        assert validate_cell("Manufacturer", "") is None


# ---------------------------------------------------------------------------
# find_duplicate_values
# ---------------------------------------------------------------------------

class TestFindDuplicateValues:
    def test_no_duplicates(self):
        rows = [{"IPN": "A"}, {"IPN": "B"}, {"IPN": "C"}]
        assert find_duplicate_values(rows, "IPN") == set()

    def test_one_pair(self):
        rows = [{"IPN": "A"}, {"IPN": "A"}, {"IPN": "B"}]
        assert find_duplicate_values(rows, "IPN") == {0, 1}

    def test_three_dupes(self):
        rows = [{"IPN": "A"}, {"IPN": "B"}, {"IPN": "A"}, {"IPN": "A"}]
        assert find_duplicate_values(rows, "IPN") == {0, 2, 3}

    def test_blanks_ignored(self):
        rows = [{"IPN": ""}, {"IPN": ""}, {"IPN": "A"}]
        assert find_duplicate_values(rows, "IPN") == set()

    def test_empty_list(self):
        assert find_duplicate_values([], "IPN") == set()
