from __future__ import annotations

import csv
from dataclasses import dataclass
from pathlib import Path


SUB_HEADERS = ["IPN", "MPN", "Manufacturer", "Datasheet", "Supplier", "SupplierPN"]


@dataclass
class SubstituteRecord:
    ipn: str
    mpn: str
    manufacturer: str
    datasheet: str
    supplier: str
    supplier_pn: str


class SubstitutesStore:
    def __init__(self, path: Path):
        self.path = path
        self.records: list[SubstituteRecord] = []
        self.load()

    def load(self) -> None:
        self.records = []
        if not self.path.exists():
            return
        with self.path.open("r", encoding="utf-8", newline="") as handle:
            reader = csv.DictReader(handle)
            for row in reader:
                self.records.append(
                    SubstituteRecord(
                        ipn=row.get("IPN", ""),
                        mpn=row.get("MPN", ""),
                        manufacturer=row.get("Manufacturer", ""),
                        datasheet=row.get("Datasheet", ""),
                        supplier=row.get("Supplier", ""),
                        supplier_pn=row.get("SupplierPN", ""),
                    )
                )

    def save(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self.path.open("w", encoding="utf-8", newline="") as handle:
            writer = csv.DictWriter(handle, fieldnames=SUB_HEADERS)
            writer.writeheader()
            for record in self.records:
                writer.writerow(
                    {
                        "IPN": record.ipn,
                        "MPN": record.mpn,
                        "Manufacturer": record.manufacturer,
                        "Datasheet": record.datasheet,
                        "Supplier": record.supplier,
                        "SupplierPN": record.supplier_pn,
                    }
                )

    def by_ipn(self, ipn: str) -> list[SubstituteRecord]:
        return [record for record in self.records if record.ipn == ipn]

    def add(self, record: SubstituteRecord) -> None:
        self.records.append(record)
        self.save()

