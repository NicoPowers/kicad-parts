from __future__ import annotations

from urllib.parse import urlparse

from .ipn import is_valid_ipn
from .schema import REQUIRED_COLUMNS


def is_valid_url(value: str) -> bool:
    if not value:
        return True
    try:
        parsed = urlparse(value.strip())
    except ValueError:
        return False
    return parsed.scheme in {"http", "https"} and bool(parsed.netloc)


def validate_cell(column: str, value: str) -> str | None:
    if column == "IPN" and not is_valid_ipn(value):
        return "Invalid IPN format; expected PP-CCC-NNNN-VVVV"
    if column in {"Datasheet"} and not is_valid_url(value):
        return "Invalid URL"
    if column in REQUIRED_COLUMNS and not value.strip():
        return f"{column} is required"
    return None


def find_duplicate_values(rows: list[dict[str, str]], column: str) -> set[int]:
    seen: dict[str, int] = {}
    dupes: set[int] = set()
    for idx, row in enumerate(rows):
        value = row.get(column, "").strip()
        if not value:
            continue
        if value in seen:
            dupes.add(seen[value])
            dupes.add(idx)
        else:
            seen[value] = idx
    return dupes

