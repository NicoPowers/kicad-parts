"""Pure SI-value and power-rating parsing — no GUI dependencies."""
from __future__ import annotations

import re

SI_INLINE_RE = re.compile(r"^\s*([+-]?\d+(?:\.\d+)?)\s*([pnumkKMGgRrµuU])\s*(\d+)\s*$")
UNIT_SUFFIX_RE = re.compile(r"(?i)\s*(ohms?|[ωΩ]|[fwhva])\s*$")
POWER_SUFFIX_RE = re.compile(r"(?i)\s*(w|watt|watts)\s*$")
SI_PREFIXES = (
    (1e-12, "p"),
    (1e-9, "n"),
    (1e-6, "u"),
    (1e-3, "m"),
    (1.0, ""),
    (1e3, "k"),
    (1e6, "M"),
    (1e9, "G"),
)


def _safe_float(text: str) -> float:
    try:
        return float(text)
    except ValueError as exc:
        raise ValueError(f"Cannot parse '{text}' as a number") from exc


def _multiplier_for_token(token: str) -> float:
    if token in {"k", "K"}:
        return 1e3
    if token in {"m"}:
        return 1e-3
    if token in {"M"}:
        return 1e6
    if token in {"g", "G"}:
        return 1e9
    if token in {"u", "U", "µ"}:
        return 1e-6
    if token in {"n", "N"}:
        return 1e-9
    if token in {"p", "P"}:
        return 1e-12
    if token in {"r", "R"}:
        return 1.0
    raise ValueError(f"Unsupported SI suffix '{token}'")


def _strip_terminal_units(raw: str) -> str:
    cleaned = raw
    while True:
        updated = UNIT_SUFFIX_RE.sub("", cleaned)
        if updated == cleaned:
            return updated.strip()
        cleaned = updated


def parse_si_value(text: str) -> float:
    raw = text.strip().replace("µ", "u")
    if not raw:
        return 0.0
    raw = _strip_terminal_units(raw)
    if not raw:
        return 0.0

    inline = SI_INLINE_RE.match(raw)
    if inline:
        whole, suffix, frac = inline.groups()
        value = _safe_float(f"{whole}.{frac}")
        return value * _multiplier_for_token(suffix)

    suffix = raw[-1]
    if suffix.isalpha():
        num_text = raw[:-1].strip()
        if not num_text:
            raise ValueError(f"Cannot parse '{text}' as a number")
        return _safe_float(num_text) * _multiplier_for_token(suffix)
    return _safe_float(raw)


def parse_power_rating(text: str) -> tuple[float, str]:
    raw = text.strip()
    if not raw:
        return 0.0, ""
    raw = POWER_SUFFIX_RE.sub("", raw).strip()
    if "/" in raw:
        left, right = raw.split("/", 1)
        watts = _safe_float(left.strip()) / _safe_float(right.strip())
    else:
        watts = parse_si_value(raw)
    if watts <= 0:
        raise ValueError("Power rating must be a positive value")
    return watts, f"{watts:g}W"


def format_si_value(value: float) -> str:
    if value == 0:
        return "0"
    sign = "-" if value < 0 else ""
    abs_value = abs(value)
    for multiplier, prefix in SI_PREFIXES:
        coefficient = abs_value / multiplier
        if 1 <= coefficient < 1000:
            rounded = float(f"{coefficient:.3g}")
            if rounded.is_integer():
                return f"{sign}{int(rounded)}{prefix}"
            return f"{sign}{rounded:g}{prefix}"
    return f"{value:g}"
