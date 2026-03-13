from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class SearchHit:
    category: str
    ipn: str
    mpn: str
    description: str


def search_rows(category: str, rows: list[dict[str, str]], query: str) -> list[SearchHit]:
    needle = query.strip().lower()
    if not needle:
        return []
    hits: list[SearchHit] = []
    for row in rows:
        hay = " ".join([row.get("IPN", ""), row.get("MPN", ""), row.get("Description", "")]).lower()
        if needle in hay:
            hits.append(
                SearchHit(
                    category=category,
                    ipn=row.get("IPN", ""),
                    mpn=row.get("MPN", ""),
                    description=row.get("Description", ""),
                )
            )
    return hits

