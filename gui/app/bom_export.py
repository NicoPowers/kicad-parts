from __future__ import annotations

import csv
from pathlib import Path

from .substitutes import SubstitutesStore


def export_bom_wide(rows: list[dict[str, str]], out_path: Path, substitutes: SubstitutesStore) -> None:
    max_subs = 0
    for row in rows:
        max_subs = max(max_subs, len(substitutes.by_ipn(row.get("IPN", ""))))
    headers = ["IPN", "Description", "Qty", "Primary_MPN", "Primary_Manufacturer"]
    for idx in range(max_subs):
        headers.extend([f"Alt{idx + 1}_MPN", f"Alt{idx + 1}_Manufacturer"])

    with out_path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=headers)
        writer.writeheader()
        for row in rows:
            ipn = row.get("IPN", "")
            data = {
                "IPN": ipn,
                "Description": row.get("Description", ""),
                "Qty": "",
                "Primary_MPN": row.get("MPN", ""),
                "Primary_Manufacturer": row.get("Manufacturer", ""),
            }
            subs = substitutes.by_ipn(ipn)
            for idx, sub in enumerate(subs):
                data[f"Alt{idx + 1}_MPN"] = sub.mpn
                data[f"Alt{idx + 1}_Manufacturer"] = sub.manufacturer
            writer.writerow(data)


def export_bom_long(rows: list[dict[str, str]], out_path: Path, substitutes: SubstitutesStore) -> None:
    headers = ["IPN", "Description", "Qty", "Role", "MPN", "Manufacturer", "Supplier", "SupplierPN"]
    with out_path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=headers)
        writer.writeheader()
        for row in rows:
            ipn = row.get("IPN", "")
            writer.writerow(
                {
                    "IPN": ipn,
                    "Description": row.get("Description", ""),
                    "Qty": "",
                    "Role": "Primary",
                    "MPN": row.get("MPN", ""),
                    "Manufacturer": row.get("Manufacturer", ""),
                    "Supplier": "",
                    "SupplierPN": "",
                }
            )
            for sub in substitutes.by_ipn(ipn):
                writer.writerow(
                    {
                        "IPN": ipn,
                        "Description": row.get("Description", ""),
                        "Qty": "",
                        "Role": "Alternate",
                        "MPN": sub.mpn,
                        "Manufacturer": sub.manufacturer,
                        "Supplier": sub.supplier,
                        "SupplierPN": sub.supplier_pn,
                    }
                )

