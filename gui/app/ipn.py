from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

from .csv_manager import read_csv
from .standard_values import encode_capacitor_code, encode_resistor_e96


IPN_PATTERN = re.compile(r"^(?P<ccc>[A-Z]{3})-(?P<nnnn>\d{4})-(?P<vvvv>[A-Z0-9]{4})$")


@dataclass(frozen=True)
class ParsedIPN:
    ccc: str
    nnnn: str
    vvvv: str


def parse_ipn(value: str) -> ParsedIPN | None:
    match = IPN_PATTERN.match(value.strip())
    if not match:
        return None
    return ParsedIPN(match.group("ccc"), match.group("nnnn"), match.group("vvvv"))


def is_valid_ipn(value: str) -> bool:
    return parse_ipn(value) is not None


def collect_all_ipns(database_dir: Path) -> set[str]:
    ipns: set[str] = set()
    for csv_path in sorted(database_dir.glob("g-*.csv")):
        doc = read_csv(csv_path)
        for row in doc.rows:
            ipn = row.get("IPN", "").strip()
            if ipn:
                ipns.add(ipn)
    return ipns


def ipn_exists(ipn: str, database_dir: Path) -> bool:
    return ipn in collect_all_ipns(database_dir)


def _next_sequence_for_ccc(ccc: str, existing_ipns: set[str]) -> str:
    seq = 0
    for ipn in existing_ipns:
        parsed = parse_ipn(ipn)
        if parsed and parsed.ccc == ccc:
            seq = max(seq, int(parsed.nnnn))
    return f"{seq + 1:04d}"


def generate_sequential_ipn(ccc: str, existing_ipns: set[str], vvvv: str = "0001") -> str:
    nnnn = _next_sequence_for_ccc(ccc, existing_ipns)
    candidate = f"{ccc}-{nnnn}-{vvvv}"
    while candidate in existing_ipns:
        nnnn = f"{int(nnnn) + 1:04d}"
        candidate = f"{ccc}-{nnnn}-{vvvv}"
    return candidate


def generate_resistor_ipn(existing_ipns: set[str], resistance_ohms: float, family: str = "0000") -> str:
    vvvv = encode_resistor_e96(resistance_ohms)
    base = f"RES-{family}-{vvvv}"
    if base not in existing_ipns:
        return base
    return generate_sequential_ipn("RES", existing_ipns, vvvv)


def generate_capacitor_ipn(existing_ipns: set[str], capacitance_farads: float, family: str = "0000") -> str:
    vvvv = encode_capacitor_code(capacitance_farads)
    base = f"CAP-{family}-{vvvv}"
    if base not in existing_ipns:
        return base
    return generate_sequential_ipn("CAP", existing_ipns, vvvv)


def generate_inductor_ipn(existing_ipns: set[str], family: str = "0000", vvvv: str = "0001") -> str:
    base = f"IND-{family}-{vvvv}"
    if base not in existing_ipns:
        return base
    return generate_sequential_ipn("IND", existing_ipns, vvvv)

