"""Tests for SI value parsing and power-rating parsing in smart_form.py."""
from __future__ import annotations

import pytest

from app.si_parser import parse_power_rating, parse_si_value


# ---------------------------------------------------------------------------
# parse_si_value – kilo
# ---------------------------------------------------------------------------

class TestSiKilo:
    def test_25k(self):
        assert parse_si_value("25k") == pytest.approx(25_000)

    def test_25K_upper(self):
        assert parse_si_value("25K") == pytest.approx(25_000)

    def test_4k7_inline(self):
        assert parse_si_value("4k7") == pytest.approx(4_700)

    def test_1k0_inline(self):
        assert parse_si_value("1k0") == pytest.approx(1_000)

    def test_10k(self):
        assert parse_si_value("10k") == pytest.approx(10_000)

    def test_100k(self):
        assert parse_si_value("100k") == pytest.approx(100_000)

    def test_2k2_inline(self):
        assert parse_si_value("2k2") == pytest.approx(2_200)


# ---------------------------------------------------------------------------
# parse_si_value – mega
# ---------------------------------------------------------------------------

class TestSiMega:
    def test_1M(self):
        assert parse_si_value("1M") == pytest.approx(1_000_000)

    def test_4M7_inline(self):
        assert parse_si_value("4M7") == pytest.approx(4_700_000)

    def test_10M(self):
        assert parse_si_value("10M") == pytest.approx(10_000_000)


# ---------------------------------------------------------------------------
# parse_si_value – milli
# ---------------------------------------------------------------------------

class TestSiMilli:
    def test_100m(self):
        assert parse_si_value("100m") == pytest.approx(0.1)

    def test_4m7_inline(self):
        assert parse_si_value("4m7") == pytest.approx(0.0047)

    def test_2_2m(self):
        assert parse_si_value("2.2m") == pytest.approx(0.0022)


# ---------------------------------------------------------------------------
# parse_si_value – micro
# ---------------------------------------------------------------------------

class TestSiMicro:
    def test_3_3u(self):
        assert parse_si_value("3.3u") == pytest.approx(3.3e-6)

    def test_47u(self):
        assert parse_si_value("47u") == pytest.approx(47e-6)

    def test_100u(self):
        assert parse_si_value("100u") == pytest.approx(100e-6)

    def test_micro_symbol(self):
        assert parse_si_value("4.7µ") == pytest.approx(4.7e-6)

    def test_uppercase_U(self):
        assert parse_si_value("10U") == pytest.approx(10e-6)

    def test_3u3_inline(self):
        assert parse_si_value("3u3") == pytest.approx(3.3e-6)


# ---------------------------------------------------------------------------
# parse_si_value – nano
# ---------------------------------------------------------------------------

class TestSiNano:
    def test_100n(self):
        assert parse_si_value("100n") == pytest.approx(100e-9)

    def test_4n7_inline(self):
        assert parse_si_value("4n7") == pytest.approx(4.7e-9)

    def test_2_2n(self):
        assert parse_si_value("2.2n") == pytest.approx(2.2e-9)

    def test_uppercase_N(self):
        assert parse_si_value("100N") == pytest.approx(100e-9)


# ---------------------------------------------------------------------------
# parse_si_value – pico
# ---------------------------------------------------------------------------

class TestSiPico:
    def test_100p(self):
        assert parse_si_value("100p") == pytest.approx(100e-12)

    def test_4p7_inline(self):
        assert parse_si_value("4p7") == pytest.approx(4.7e-12)

    def test_22p(self):
        assert parse_si_value("22p") == pytest.approx(22e-12)


# ---------------------------------------------------------------------------
# parse_si_value – giga
# ---------------------------------------------------------------------------

class TestSiGiga:
    def test_1G(self):
        assert parse_si_value("1G") == pytest.approx(1e9)

    def test_2g2_inline(self):
        assert parse_si_value("2G2") == pytest.approx(2.2e9)


# ---------------------------------------------------------------------------
# parse_si_value – plain numbers
# ---------------------------------------------------------------------------

class TestSiPlain:
    def test_integer(self):
        assert parse_si_value("470") == pytest.approx(470)

    def test_float(self):
        assert parse_si_value("4.7") == pytest.approx(4.7)

    def test_zero(self):
        assert parse_si_value("0") == pytest.approx(0.0)

    def test_empty(self):
        assert parse_si_value("") == pytest.approx(0.0)

    def test_whitespace(self):
        assert parse_si_value("   ") == pytest.approx(0.0)

    def test_plain_large(self):
        assert parse_si_value("1000000") == pytest.approx(1_000_000)


# ---------------------------------------------------------------------------
# parse_si_value – R designator (e.g. 4R7 = 4.7 Ω)
# ---------------------------------------------------------------------------

class TestSiR:
    def test_4R7(self):
        assert parse_si_value("4R7") == pytest.approx(4.7)

    def test_1R0(self):
        assert parse_si_value("1R0") == pytest.approx(1.0)

    def test_0R47(self):
        """0R47 is not inline notation (leading zero), falls to suffix path."""
        assert parse_si_value("0R47") == pytest.approx(0.47)


# ---------------------------------------------------------------------------
# parse_si_value – unit suffix stripping
# ---------------------------------------------------------------------------

class TestSiUnitStripping:
    def test_ohm_suffix(self):
        assert parse_si_value("10kohm") == pytest.approx(10_000)

    def test_ohms_suffix(self):
        assert parse_si_value("4.7kohms") == pytest.approx(4_700)

    def test_omega_suffix(self):
        assert parse_si_value("100Ω") == pytest.approx(100)

    def test_farad_suffix(self):
        assert parse_si_value("3.3uF") == pytest.approx(3.3e-6)

    def test_henry_suffix(self):
        assert parse_si_value("10uH") == pytest.approx(10e-6)

    def test_watt_suffix_on_value(self):
        assert parse_si_value("100mW") == pytest.approx(0.1)

    def test_volt_suffix(self):
        assert parse_si_value("3.3V") == pytest.approx(3.3)

    def test_amp_suffix(self):
        assert parse_si_value("500mA") == pytest.approx(0.5)


# ---------------------------------------------------------------------------
# parse_si_value – error handling
# ---------------------------------------------------------------------------

class TestSiErrors:
    def test_bare_letter(self):
        with pytest.raises(ValueError):
            parse_si_value("k")

    def test_garbage(self):
        with pytest.raises(ValueError):
            parse_si_value("abc")

    def test_unsupported_suffix(self):
        with pytest.raises(ValueError):
            parse_si_value("10x")


# ---------------------------------------------------------------------------
# parse_power_rating
# ---------------------------------------------------------------------------

class TestParsePowerRating:
    def test_100m(self):
        watts, label = parse_power_rating("100m")
        assert watts == pytest.approx(0.1)
        assert label == "0.1W"

    def test_100mW(self):
        watts, label = parse_power_rating("100mW")
        assert watts == pytest.approx(0.1)
        assert label == "0.1W"

    def test_100mwatt(self):
        watts, label = parse_power_rating("100mwatt")
        assert watts == pytest.approx(0.1)

    def test_quarter_watt(self):
        watts, label = parse_power_rating("1/4W")
        assert watts == pytest.approx(0.25)
        assert label == "0.25W"

    def test_tenth_watt(self):
        watts, label = parse_power_rating("1/10W")
        assert watts == pytest.approx(0.1)

    def test_fraction_no_suffix(self):
        watts, label = parse_power_rating("1/8")
        assert watts == pytest.approx(0.125)

    def test_1W(self):
        watts, label = parse_power_rating("1W")
        assert watts == pytest.approx(1.0)
        assert label == "1W"

    def test_0_5W(self):
        watts, label = parse_power_rating("0.5W")
        assert watts == pytest.approx(0.5)

    def test_empty(self):
        watts, label = parse_power_rating("")
        assert watts == 0.0
        assert label == ""

    def test_negative_raises(self):
        with pytest.raises(ValueError, match="positive"):
            parse_power_rating("-1")
