from __future__ import annotations

from dataclasses import dataclass
from math import inf, log10


E12_BASE = [1.0, 1.2, 1.5, 1.8, 2.2, 2.7, 3.3, 3.9, 4.7, 5.6, 6.8, 8.2]
E24_BASE = [
    1.0,
    1.1,
    1.2,
    1.3,
    1.5,
    1.6,
    1.8,
    2.0,
    2.2,
    2.4,
    2.7,
    3.0,
    3.3,
    3.6,
    3.9,
    4.3,
    4.7,
    5.1,
    5.6,
    6.2,
    6.8,
    7.5,
    8.2,
    9.1,
]

# IEC 60063 E96 significant values as integers.
E96_NUMBERS = [
    100,
    102,
    105,
    107,
    110,
    113,
    115,
    118,
    121,
    124,
    127,
    130,
    133,
    137,
    140,
    143,
    147,
    150,
    154,
    158,
    162,
    165,
    169,
    174,
    178,
    182,
    187,
    191,
    196,
    200,
    205,
    210,
    215,
    221,
    226,
    232,
    237,
    243,
    249,
    255,
    261,
    267,
    274,
    280,
    287,
    294,
    301,
    309,
    316,
    324,
    332,
    340,
    348,
    357,
    365,
    374,
    383,
    392,
    402,
    412,
    422,
    432,
    442,
    453,
    464,
    475,
    487,
    499,
    511,
    523,
    536,
    549,
    562,
    576,
    590,
    604,
    619,
    634,
    649,
    665,
    681,
    698,
    715,
    732,
    750,
    768,
    787,
    806,
    825,
    845,
    866,
    887,
    909,
    931,
    953,
    976,
]


@dataclass(frozen=True)
class SnapResult:
    value: float
    code: str
    error_percent: float


def _expand_series(base_values: list[float], min_exp: int = -12, max_exp: int = 9) -> list[float]:
    out: list[float] = []
    for exp in range(min_exp, max_exp + 1):
        scale = 10**exp
        for base in base_values:
            out.append(base * scale)
    return out


def snap_to_nearest(value: float, series: str) -> SnapResult:
    if value <= 0:
        return SnapResult(value=0.0, code="0000", error_percent=100.0)

    candidates: list[float]
    if series == "E96":
        candidates = [n / 100 for n in E96_NUMBERS]
    elif series == "E24":
        candidates = E24_BASE
    elif series == "E12":
        candidates = E12_BASE
    else:
        raise ValueError(f"Unknown series: {series}")

    decade = int(log10(value))
    best = None
    best_err = inf
    for d in range(decade - 2, decade + 3):
        mul = 10**d
        for candidate in candidates:
            c = candidate * mul
            err = abs(c - value)
            if err < best_err:
                best_err = err
                best = c
    assert best is not None
    error_pct = abs(best - value) / value * 100.0
    return SnapResult(value=best, code=encode_resistor_e96(best), error_percent=error_pct)


def encode_resistor_e96(value_ohms: float) -> str:
    """Return the 4-digit style seen in this repo for resistor Value code."""
    if value_ohms <= 0:
        return "0000"
    # Use significant value + decade exponent.
    exp = 0
    norm = value_ohms
    while norm >= 100:
        norm /= 10
        exp += 1
    while norm < 10:
        norm *= 10
        exp -= 1
    sig = int(round(norm))
    # Last digit is exponent offset by 1 to fit existing examples (1k -> 1001).
    return f"{sig:03d}{exp + 1:d}"


def encode_capacitor_code(value_farads: float) -> str:
    """Return 4-digit capacitor code, last 3 from pF code, prefixed with 0."""
    if value_farads <= 0:
        return "0000"
    pf = value_farads * 1e12
    if pf < 10:
        # For very small caps keep rounded integer.
        return f"0{int(round(pf)):03d}"
    exp = 0
    sig = pf
    while sig >= 100:
        sig /= 10
        exp += 1
    sig_i = int(round(sig))
    return f"0{sig_i:02d}{exp:d}"


def snap_resistor(value_ohms: float, tolerance: str) -> SnapResult:
    series = "E96" if "1%" in tolerance else "E24"
    result = snap_to_nearest(value_ohms, series)
    return SnapResult(value=result.value, code=encode_resistor_e96(result.value), error_percent=result.error_percent)


def snap_capacitor(value_farads: float, tolerance: str) -> SnapResult:
    series = "E24" if "5%" in tolerance else "E12"
    result = snap_to_nearest(value_farads, series)
    return SnapResult(value=result.value, code=encode_capacitor_code(result.value), error_percent=result.error_percent)


def snap_inductor(value_henry: float) -> SnapResult:
    result = snap_to_nearest(value_henry, "E12")
    # Inductors in this repo do not currently encode strict series; keep 4-digit style.
    return SnapResult(value=result.value, code=encode_capacitor_code(result.value), error_percent=result.error_percent)

