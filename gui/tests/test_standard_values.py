"""Tests for standard-value snapping and encoding in standard_values.py."""
from __future__ import annotations

import pytest

from app.standard_values import (
    encode_capacitor_code,
    encode_resistor_e96,
    snap_capacitor,
    snap_inductor,
    snap_resistor,
    snap_to_nearest,
)


# ---------------------------------------------------------------------------
# snap_to_nearest
# ---------------------------------------------------------------------------

class TestSnapToNearest:
    def test_exact_E96_value(self):
        result = snap_to_nearest(10_000, "E96")
        assert result.value == pytest.approx(10_000)
        assert result.error_percent == pytest.approx(0.0, abs=0.01)

    def test_close_E96_value(self):
        result = snap_to_nearest(4_800, "E96")
        assert result.value == pytest.approx(4_870, rel=0.05)

    def test_E24_1k(self):
        result = snap_to_nearest(1_000, "E24")
        assert result.value == pytest.approx(1_000)

    def test_E12_4_7(self):
        result = snap_to_nearest(4.7, "E12")
        assert result.value == pytest.approx(4.7)

    def test_zero_value(self):
        result = snap_to_nearest(0, "E96")
        assert result.value == 0.0
        assert result.error_percent == 100.0

    def test_unknown_series_raises(self):
        with pytest.raises(ValueError, match="Unknown series"):
            snap_to_nearest(100, "E48")

    def test_small_value_pico_range(self):
        result = snap_to_nearest(4.7e-12, "E12")
        assert result.value == pytest.approx(4.7e-12, rel=0.01)


# ---------------------------------------------------------------------------
# encode_resistor_e96
# ---------------------------------------------------------------------------

class TestEncodeResistorE96:
    def test_1k(self):
        code = encode_resistor_e96(1_000)
        assert code == "0103"

    def test_10k(self):
        code = encode_resistor_e96(10_000)
        assert code == "0104"

    def test_100(self):
        code = encode_resistor_e96(100)
        assert code == "0102"

    def test_4_7(self):
        code = encode_resistor_e96(4.7)
        assert code == "0470"

    def test_zero(self):
        assert encode_resistor_e96(0) == "0000"

    def test_deterministic(self):
        assert encode_resistor_e96(1_000) == encode_resistor_e96(1_000)


# ---------------------------------------------------------------------------
# encode_capacitor_code
# ---------------------------------------------------------------------------

class TestEncodeCapacitorCode:
    def test_100nF(self):
        code = encode_capacitor_code(100e-9)
        assert code == "0104"

    def test_10uF(self):
        code = encode_capacitor_code(10e-6)
        assert code == "0106"

    def test_1pF(self):
        code = encode_capacitor_code(1e-12)
        assert code == "0001"

    def test_zero(self):
        assert encode_capacitor_code(0) == "0000"

    def test_22pF(self):
        code = encode_capacitor_code(22e-12)
        assert code == "0220"


# ---------------------------------------------------------------------------
# snap_resistor
# ---------------------------------------------------------------------------

class TestSnapResistor:
    def test_1_percent_uses_E96(self):
        result = snap_resistor(4_800, "1%")
        assert result.value == pytest.approx(4_870, rel=0.05)

    def test_5_percent_uses_E24(self):
        result = snap_resistor(4_800, "5%")
        assert result.value == pytest.approx(5_100, rel=0.1)

    def test_10_percent_uses_E24(self):
        result = snap_resistor(4_800, "10%")
        assert result.value == pytest.approx(5_100, rel=0.1)


# ---------------------------------------------------------------------------
# snap_capacitor
# ---------------------------------------------------------------------------

class TestSnapCapacitor:
    def test_5_percent_uses_E24(self):
        result = snap_capacitor(3.3e-6, "5%")
        assert result.value == pytest.approx(3.3e-6, rel=0.05)

    def test_10_percent_uses_E12(self):
        result = snap_capacitor(3.3e-6, "10%")
        assert result.value == pytest.approx(3.3e-6, rel=0.05)


# ---------------------------------------------------------------------------
# snap_inductor
# ---------------------------------------------------------------------------

class TestSnapInductor:
    def test_10uH(self):
        result = snap_inductor(10e-6)
        assert result.value == pytest.approx(10e-6, rel=0.05)

    def test_47uH(self):
        result = snap_inductor(47e-6)
        assert result.value == pytest.approx(47e-6, rel=0.05)
