"""Tests for IPN parsing, validation, and generation in ipn.py."""
from __future__ import annotations

import pytest

from app.ipn import (
    ParsedIPN,
    generate_capacitor_ipn,
    generate_inductor_ipn,
    generate_resistor_ipn,
    generate_sequential_ipn,
    is_valid_ipn,
    parse_ipn,
)


# ---------------------------------------------------------------------------
# parse_ipn / is_valid_ipn
# ---------------------------------------------------------------------------

class TestParseIPN:
    def test_valid_ipn(self):
        result = parse_ipn("SL-RES-0001-1004")
        assert result == ParsedIPN("SL", "RES", "0001", "1004")

    def test_valid_ipn_alpha_vvvv(self):
        result = parse_ipn("MA-CAP-0012-A3B1")
        assert result == ParsedIPN("MA", "CAP", "0012", "A3B1")

    def test_lowercase_rejected(self):
        assert parse_ipn("sl-res-0001-1004") is None

    def test_wrong_format(self):
        assert parse_ipn("RESISTOR-1-2") is None

    def test_empty(self):
        assert parse_ipn("") is None

    def test_whitespace_stripped(self):
        result = parse_ipn("  SL-RES-0001-1004  ")
        assert result is not None
        assert result.ccc == "RES"


class TestIsValidIPN:
    def test_valid(self):
        assert is_valid_ipn("SL-RES-0001-1004") is True

    def test_invalid(self):
        assert is_valid_ipn("not-an-ipn") is False

    def test_empty(self):
        assert is_valid_ipn("") is False


# ---------------------------------------------------------------------------
# generate_sequential_ipn
# ---------------------------------------------------------------------------

class TestGenerateSequentialIPN:
    def test_empty_existing(self):
        ipn = generate_sequential_ipn("SL", "RES", set())
        assert ipn == "SL-RES-0001-0001"

    def test_increments_past_existing(self):
        existing = {"SL-RES-0001-1004", "SL-RES-0003-1005"}
        ipn = generate_sequential_ipn("SL", "RES", existing)
        assert ipn == "SL-RES-0004-0001"

    def test_skips_collision(self):
        existing = {"SL-RES-0001-0001", "SL-RES-0002-0001"}
        ipn = generate_sequential_ipn("SL", "RES", existing)
        assert ipn == "SL-RES-0003-0001"

    def test_ignores_other_category(self):
        existing = {"SL-CAP-0010-0104", "MA-RES-0003-0001"}
        ipn = generate_sequential_ipn("SL", "RES", existing)
        assert ipn == "SL-RES-0001-0001"


# ---------------------------------------------------------------------------
# generate_resistor_ipn
# ---------------------------------------------------------------------------

class TestGenerateResistorIPN:
    def test_1k_first(self):
        ipn = generate_resistor_ipn("SL", set(), 1000)
        assert ipn.startswith("SL-RES-")
        assert is_valid_ipn(ipn)

    def test_collision_fallback(self):
        first = generate_resistor_ipn("SL", set(), 1000)
        second = generate_resistor_ipn("SL", {first}, 1000)
        assert second != first
        assert is_valid_ipn(second)


# ---------------------------------------------------------------------------
# generate_capacitor_ipn
# ---------------------------------------------------------------------------

class TestGenerateCapacitorIPN:
    def test_100nF(self):
        ipn = generate_capacitor_ipn("SL", set(), 100e-9)
        assert ipn.startswith("SL-CAP-")
        assert is_valid_ipn(ipn)

    def test_collision(self):
        first = generate_capacitor_ipn("SL", set(), 100e-9)
        second = generate_capacitor_ipn("SL", {first}, 100e-9)
        assert second != first


# ---------------------------------------------------------------------------
# generate_inductor_ipn
# ---------------------------------------------------------------------------

class TestGenerateInductorIPN:
    def test_default(self):
        ipn = generate_inductor_ipn("SL", set())
        assert ipn == "SL-IND-0000-0001"

    def test_collision(self):
        first = generate_inductor_ipn("SL", set())
        second = generate_inductor_ipn("SL", {first})
        assert second != first
        assert is_valid_ipn(second)
