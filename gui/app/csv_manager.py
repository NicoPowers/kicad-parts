from __future__ import annotations

import csv
import shutil
from dataclasses import dataclass
from pathlib import Path


@dataclass
class CsvDocument:
    path: Path
    headers: list[str]
    rows: list[dict[str, str]]
    quote_all: bool


def _detect_quote_all(path: Path) -> bool:
    first_line = path.read_text(encoding="utf-8", errors="ignore").splitlines()
    if not first_line:
        return False
    return first_line[0].strip().startswith('"')


def read_csv(path: Path) -> CsvDocument:
    with path.open("r", encoding="utf-8", newline="") as handle:
        reader = csv.DictReader(handle)
        headers = list(reader.fieldnames or [])
        rows = []
        for row in reader:
            normalized: dict[str, str] = {}
            for header in headers:
                value = row.get(header, "")
                normalized[header] = "" if value is None else str(value)
            rows.append(normalized)
    return CsvDocument(path=path, headers=headers, rows=rows, quote_all=_detect_quote_all(path))


def backup_file(path: Path) -> Path:
    backup = path.with_suffix(path.suffix + ".bak")
    shutil.copy2(path, backup)
    return backup


def sort_rows_by_ipn(rows: list[dict[str, str]]) -> list[dict[str, str]]:
    return sorted(rows, key=lambda row: row.get("IPN", ""))


def write_csv(document: CsvDocument, make_backup: bool = True) -> None:
    if make_backup and document.path.exists():
        backup_file(document.path)
    rows = sort_rows_by_ipn(document.rows)
    quoting = csv.QUOTE_ALL if document.quote_all else csv.QUOTE_MINIMAL
    with document.path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=document.headers, quoting=quoting)
        writer.writeheader()
        for row in rows:
            writer.writerow({h: row.get(h, "") for h in document.headers})

