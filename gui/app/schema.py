from __future__ import annotations

import csv
from dataclasses import dataclass
from pathlib import Path


URL_COLUMNS = {"Datasheet", "LCSC", "DigiKey_PN", "Mouser_PN"}
PRICE_COLUMNS = {"DigiKey_Price", "Mouser_Price", "Price_Range", "Price_LastSynced_UTC"}
REQUIRED_COLUMNS = {"IPN", "Description", "Symbol", "Footprint"}
NUMERIC_HINTS = {
    "Value",
    "Current",
    "Voltage",
    "max(Vce)",
    "max(Ic)",
    "Pins",
}


@dataclass(frozen=True)
class CategorySchema:
    key: str
    csv_path: Path
    headers: list[str]

    @property
    def required_columns(self) -> list[str]:
        return [col for col in self.headers if col in REQUIRED_COLUMNS]


def category_from_filename(csv_path: Path) -> str:
    return csv_path.stem.removeprefix("g-")


def discover_category_schemas(database_dir: Path) -> list[CategorySchema]:
    schemas: list[CategorySchema] = []
    for csv_path in sorted(database_dir.glob("g-*.csv")):
        with csv_path.open("r", encoding="utf-8", newline="") as handle:
            reader = csv.reader(handle)
            headers = next(reader, [])
        schemas.append(
            CategorySchema(
                key=category_from_filename(csv_path),
                csv_path=csv_path,
                headers=headers,
            )
        )
    return schemas


def infer_column_type(column: str) -> str:
    if column in URL_COLUMNS:
        return "url"
    if column in NUMERIC_HINTS:
        return "number"
    return "text"


def default_row_for_headers(headers: list[str]) -> dict[str, str]:
    row = {header: "" for header in headers}
    for fixed in ("Symbol", "Footprint", "Manufacturer"):
        if fixed in row:
            row[fixed] = ""
    if "Price_Range" in row and not row["Price_Range"]:
        row["Price_Range"] = "?"
    return row

