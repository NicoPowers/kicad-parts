from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from difflib import SequenceMatcher
import re
from typing import Generic, Literal, TypeVar

from .si_parser import parse_si_value


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


@dataclass(frozen=True)
class LocalSearchHit:
    category: str
    ipn: str
    mpn: str
    description: str
    value: str
    manufacturer: str
    datasheet: str
    digikey_pn: str
    mouser_pn: str
    digikey_price: str
    mouser_price: str
    price_range: str
    price_last_synced_utc: str
    score: float


SearchConfidence = Literal["none", "low", "medium", "high"]


@dataclass(frozen=True)
class LocalSearchSummary:
    query: str
    hits: list[LocalSearchHit]
    confidence: SearchConfidence
    best_score: float


RemoteResultT = TypeVar("RemoteResultT")


@dataclass(frozen=True)
class UnifiedSearchResult(Generic[RemoteResultT]):
    local: LocalSearchSummary
    remote_requested: bool
    remote_results: list[RemoteResultT]


def _safe_ratio(left: str, right: str) -> float:
    if not left or not right:
        return 0.0
    return SequenceMatcher(None, left, right).ratio()


def _tokenize(value: str) -> list[str]:
    return [token for token in re.split(r"[^a-z0-9]+", value.lower()) if token]


def _si_values_equivalent(query_value: float, candidate_text: str) -> bool:
    try:
        candidate_value = parse_si_value(candidate_text)
    except Exception:
        return False
    if query_value == 0 or candidate_value == 0:
        return abs(query_value - candidate_value) <= 1e-18
    return abs(query_value - candidate_value) / max(abs(query_value), abs(candidate_value)) <= 0.001


def _row_score(row: dict[str, str], query: str, parsed_query: float | None) -> float:
    query_text = query.strip().lower()
    if not query_text:
        return 0.0
    query_tokens = _tokenize(query_text)

    ipn = row.get("IPN", "").strip().lower()
    mpn = row.get("MPN", "").strip().lower()
    description = row.get("Description", "").strip().lower()
    value = row.get("Value", "").strip().lower()
    manufacturer = row.get("Manufacturer", "").strip().lower()
    blob = " ".join([ipn, mpn, description, value, manufacturer]).strip()

    score = 0.0
    if query_text == ipn or query_text == mpn:
        score = 1.0
    if query_text in ipn or query_text in mpn:
        score = max(score, 0.92)
    if query_text in description:
        score = max(score, 0.85)
    if query_text in value:
        score = max(score, 0.8)
    if query_text in manufacturer:
        score = max(score, 0.78)
    if query_text in blob:
        score = max(score, 0.72)

    if query_tokens:
        token_hits = 0.0
        for token in query_tokens:
            if token in ipn or token in mpn:
                token_hits += 1.0
            elif token in description:
                token_hits += 0.9
            elif token in value:
                token_hits += 0.8
            elif token in manufacturer:
                token_hits += 0.7
        token_score = token_hits / len(query_tokens)
        score = max(score, min(0.95, 0.35 + token_score * 0.55))

    ratio_score = max(
        _safe_ratio(query_text, ipn),
        _safe_ratio(query_text, mpn),
        _safe_ratio(query_text, description),
        _safe_ratio(query_text, value),
        _safe_ratio(query_text, manufacturer),
        _safe_ratio(query_text, blob),
    )
    score = max(score, ratio_score * 0.75)

    if parsed_query is not None and value and _si_values_equivalent(parsed_query, value):
        score = max(score, 0.88)

    return min(score, 1.0)


def rank_rows(category: str, rows: Sequence[dict[str, str]], query: str, min_score: float = 0.6) -> list[LocalSearchHit]:
    query_text = query.strip()
    if not query_text:
        return []
    parsed_query: float | None = None
    try:
        parsed_query = parse_si_value(query_text)
    except Exception:
        parsed_query = None

    ranked: list[LocalSearchHit] = []
    for row in rows:
        score = _row_score(row, query_text, parsed_query)
        if score < min_score:
            continue
        ranked.append(
            LocalSearchHit(
                category=category,
                ipn=row.get("IPN", ""),
                mpn=row.get("MPN", ""),
                description=row.get("Description", ""),
                value=row.get("Value", ""),
                manufacturer=row.get("Manufacturer", ""),
                datasheet=row.get("Datasheet", ""),
                digikey_pn=row.get("DigiKey_PN", ""),
                mouser_pn=row.get("Mouser_PN", ""),
                digikey_price=row.get("DigiKey_Price", ""),
                mouser_price=row.get("Mouser_Price", ""),
                price_range=row.get("Price_Range", ""),
                price_last_synced_utc=row.get("Price_LastSynced_UTC", ""),
                score=score,
            )
        )
    ranked.sort(key=lambda hit: (hit.score, hit.ipn, hit.mpn), reverse=True)
    return ranked


def _confidence_for_hits(hits: Sequence[LocalSearchHit]) -> SearchConfidence:
    if not hits:
        return "none"
    top = hits[0].score
    second = hits[1].score if len(hits) > 1 else 0.0
    gap = top - second
    if top >= 0.9 and gap >= 0.1:
        return "high"
    if top >= 0.86:
        return "high"
    if top >= 0.72 and gap >= 0.12:
        return "medium"
    if top >= 0.68:
        return "medium"
    return "low"


def search_local_inventory(
    categories: Mapping[str, Sequence[dict[str, str]]],
    query: str,
    *,
    min_score: float = 0.6,
    limit: int = 50,
) -> LocalSearchSummary:
    query_text = query.strip()
    if not query_text:
        return LocalSearchSummary(query=query_text, hits=[], confidence="none", best_score=0.0)

    hits: list[LocalSearchHit] = []
    for category, rows in categories.items():
        hits.extend(rank_rows(category, rows, query_text, min_score=min_score))
    hits.sort(key=lambda hit: (hit.score, hit.ipn, hit.mpn), reverse=True)
    bounded_hits = hits[:limit]
    confidence = _confidence_for_hits(bounded_hits)
    best_score = bounded_hits[0].score if bounded_hits else 0.0
    return LocalSearchSummary(query=query_text, hits=bounded_hits, confidence=confidence, best_score=best_score)


def should_search_remote(summary: LocalSearchSummary, *, force_remote: bool = False) -> bool:
    if force_remote:
        return True
    return summary.confidence in {"none", "low", "medium"}


def search_components(
    query: str,
    categories: Mapping[str, Sequence[dict[str, str]]],
    remote_search: Callable[[str, int], Sequence[RemoteResultT]],
    *,
    force_remote: bool = False,
    local_limit: int = 50,
    remote_limit: int = 20,
) -> UnifiedSearchResult[RemoteResultT]:
    local = search_local_inventory(categories, query, limit=local_limit)
    remote_requested = should_search_remote(local, force_remote=force_remote)
    remote_results = list(remote_search(query, remote_limit)) if remote_requested else []
    return UnifiedSearchResult(
        local=local,
        remote_requested=remote_requested,
        remote_results=remote_results,
    )

