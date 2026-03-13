from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import json
import re
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import quote

import requests
from dotenv import dotenv_values


@dataclass
class SupplierPart:
    source: str
    mpn: str
    manufacturer: str
    description: str
    datasheet: str
    digikey_pn: str
    mouser_pn: str
    price: str
    quantity_available: int = 0
    product_url: str = ""


class SupplierApiClient:
    def __init__(self, secrets_path: Path):
        self.config = dotenv_values(secrets_path)
        self._digikey_token = ""
        self._digikey_expiry = 0.0
        self._log_path = secrets_path.parent / "logs" / "supplier_api.log"
        self._log_lock = threading.Lock()
        self._log_path.parent.mkdir(parents=True, exist_ok=True)

    def _log_event(self, provider: str, event: str, data: dict) -> None:
        payload = {
            "ts": datetime.now(timezone.utc).isoformat(),
            "provider": provider,
            "event": event,
            "data": data,
        }
        line = json.dumps(payload, ensure_ascii=True)
        with self._log_lock:
            with self._log_path.open("a", encoding="utf-8") as handle:
                handle.write(line + "\n")

    def _get_digikey_token(self) -> str:
        now = time.time()
        if self._digikey_token and now < self._digikey_expiry - 60:
            return self._digikey_token
        client_id = self.config.get("DIGIKEY_CLIENT_ID", "")
        client_secret = self.config.get("DIGIKEY_CLIENT_SECRET", "")
        if not client_id or not client_secret:
            return ""
        response = requests.post(
            "https://api.digikey.com/v1/oauth2/token",
            data={
                "client_id": client_id,
                "client_secret": client_secret,
                "grant_type": "client_credentials",
            },
            timeout=20,
        )
        response.raise_for_status()
        payload = response.json()
        self._digikey_token = payload.get("access_token", "")
        self._digikey_expiry = now + int(payload.get("expires_in", 1800))
        return self._digikey_token

    @staticmethod
    def _safe_int(value) -> int:
        try:
            return int(value)
        except (TypeError, ValueError):
            return 0

    @staticmethod
    def _safe_float(value) -> float | None:
        try:
            return float(value)
        except (TypeError, ValueError):
            return None

    @staticmethod
    def _parse_quantity_from_text(raw: str) -> int:
        if not raw:
            return 0
        match = re.search(r"(\d[\d,]*)", raw)
        if not match:
            return 0
        return SupplierApiClient._safe_int(match.group(1).replace(",", ""))

    @staticmethod
    def _format_usd(value: float | None) -> str:
        if value is None:
            return ""
        return f"${value:.4f}".rstrip("0").rstrip(".")

    @staticmethod
    def _normalize_query(query: str) -> str:
        return " ".join(query.strip().split())

    @staticmethod
    def _clean_supplier_pn(value: str) -> str:
        text = (value or "").strip()
        if not text:
            return ""
        if text.lower() in {"n/a", "na", "none", "null", "not available", "not found", "-", "--"}:
            return ""
        return text

    def _digikey_headers(self, token: str, client_id: str) -> dict[str, str]:
        return {
            "Authorization": f"Bearer {token}",
            "X-DIGIKEY-Client-Id": client_id,
            # Locale headers are optional but improve deterministic matching/pricing behavior.
            "X-DIGIKEY-Locale-Site": self.config.get("DIGIKEY_LOCALE_SITE", "US"),
            "X-DIGIKEY-Locale-Language": self.config.get("DIGIKEY_LOCALE_LANGUAGE", "en"),
            "X-DIGIKEY-Locale-Currency": self.config.get("DIGIKEY_LOCALE_CURRENCY", "USD"),
            "Accept": "application/json",
        }

    @staticmethod
    def _mouser_records(limit: int) -> int:
        # Mouser Search API supports up to 50 records.
        return max(1, min(int(limit), 50))

    def _mouser_post(self, path: str, body: dict) -> dict:
        api_key = self.config.get("MOUSER_SEARCH_API_KEY", "")
        if not api_key:
            self._log_event("mouser", "missing_api_key", {"path": path})
            return {}
        # Prefer the documented V1 endpoint and keep V1.0 as a compatibility fallback.
        urls = [
            f"https://api.mouser.com/api/v1/{path}?apiKey={api_key}",
            f"https://api.mouser.com/api/v1.0/{path}?apiKey={api_key}",
        ]
        for url in urls:
            log_url = url.split("?")[0]
            try:
                response = requests.post(url, json=body, timeout=20)
                self._log_event(
                    "mouser",
                    "response",
                    {
                        "url": log_url,
                        "request_body": body,
                        "status_code": response.status_code,
                        "response_text": response.text,
                    },
                )
            except Exception as exc:
                self._log_event(
                    "mouser",
                    "request_error",
                    {"url": log_url, "request_body": body, "error": str(exc)},
                )
                continue
            if response.status_code >= 400:
                continue
            try:
                payload = response.json()
            except ValueError as exc:
                self._log_event(
                    "mouser",
                    "json_decode_error",
                    {"url": log_url, "error": str(exc)},
                )
                continue
            if isinstance(payload, dict):
                return payload
        self._log_event("mouser", "all_attempts_failed", {"path": path, "request_body": body})
        return {}

    def _mouser_results_from_payload(self, payload: dict) -> list[SupplierPart]:
        parts = (((payload.get("SearchResults") or {}).get("Parts")) or [])
        results: list[SupplierPart] = []
        for item in parts:
            manufacturer_pn = (item.get("ManufacturerPartNumber", "") or "").strip()
            mouser_pn = self._clean_supplier_pn(item.get("MouserPartNumber", ""))
            if not mouser_pn:
                # Some Mouser records return "N/A" for MouserPartNumber.
                # Fall back to MPN so assignment/open/sync workflows stay usable.
                mouser_pn = self._clean_supplier_pn(manufacturer_pn)
            results.append(
                SupplierPart(
                    source="Mouser",
                    mpn=manufacturer_pn,
                    manufacturer=item.get("Manufacturer", ""),
                    description=item.get("Description", ""),
                    datasheet=item.get("DataSheetUrl", ""),
                    digikey_pn="",
                    mouser_pn=mouser_pn,
                    price=item.get("PriceBreaks", [{}])[0].get("Price", "") if item.get("PriceBreaks") else "",
                    quantity_available=self._safe_int(item.get("AvailabilityInStock", 0))
                    or self._parse_quantity_from_text(item.get("Availability", "")),
                    product_url=item.get("ProductDetailUrl", ""),
                )
            )
        return results

    def _dedupe_supplier_parts(self, results: list[SupplierPart]) -> list[SupplierPart]:
        seen: set[tuple[str, str, str]] = set()
        unique: list[SupplierPart] = []
        for part in results:
            key = (
                part.source.strip().lower(),
                part.mouser_pn.strip().lower() or part.mpn.strip().lower(),
                part.mpn.strip().lower(),
            )
            if key in seen:
                continue
            seen.add(key)
            unique.append(part)
        return unique

    @staticmethod
    def _normalized_pn(value: str) -> str:
        return re.sub(r"[^a-z0-9]", "", value.lower())

    def _pick_best_part(self, parts: list[SupplierPart], supplier_pn: str, mpn_hint: str = "") -> SupplierPart | None:
        if not parts:
            return None
        normalized_supplier = self._normalized_pn(supplier_pn)
        normalized_mpn = self._normalized_pn(mpn_hint)
        if normalized_supplier:
            for part in parts:
                if self._normalized_pn(part.digikey_pn) == normalized_supplier:
                    return part
                if self._normalized_pn(part.mouser_pn) == normalized_supplier:
                    return part
        if normalized_mpn:
            for part in parts:
                if self._normalized_pn(part.mpn) == normalized_mpn:
                    return part
        for part in parts:
            if part.price:
                return part
        return parts[0]

    def _clean_price_text(self, price: str) -> str:
        text = (price or "").strip()
        if not text:
            return ""
        numeric = self._safe_float(re.sub(r"[^0-9.\-]", "", text))
        if numeric is None:
            return text
        return self._format_usd(numeric)

    def build_price_range(self, digikey_price: str, mouser_price: str) -> str:
        dk = self._clean_price_text(digikey_price)
        mou = self._clean_price_text(mouser_price)
        prices = [price for price in (dk, mou) if price]
        if not prices:
            return "?"
        if len(prices) == 1:
            return prices[0]
        left_value = self._safe_float(re.sub(r"[^0-9.\-]", "", prices[0]))
        right_value = self._safe_float(re.sub(r"[^0-9.\-]", "", prices[1]))
        if left_value is not None and right_value is not None:
            low = self._format_usd(min(left_value, right_value))
            high = self._format_usd(max(left_value, right_value))
            return low if low == high else f"{low} - {high}"
        return prices[0] if prices[0] == prices[1] else f"{prices[0]} - {prices[1]}"

    def fetch_supplier_prices(self, digikey_pn: str, mouser_pn: str, mpn_hint: str = "") -> dict[str, str]:
        digikey_price = ""
        mouser_price = ""
        digikey_query = self._normalize_query(digikey_pn)
        mouser_query = self._normalize_query(mouser_pn)
        if digikey_query:
            digikey_results = self.search_digikey_keyword(digikey_query, limit=10)
            picked = self._pick_best_part(digikey_results, digikey_query, mpn_hint=mpn_hint)
            digikey_price = picked.price if picked else ""
        if mouser_query:
            mouser_results = self.search_mouser_partnumber(mouser_query, limit=10)
            if not mouser_results:
                mouser_results = self.search_mouser_keyword(mouser_query, limit=10)
            picked = self._pick_best_part(mouser_results, mouser_query, mpn_hint=mpn_hint)
            mouser_price = picked.price if picked else ""
        digikey_price = self._clean_price_text(digikey_price)
        mouser_price = self._clean_price_text(mouser_price)
        return {
            "DigiKey_Price": digikey_price,
            "Mouser_Price": mouser_price,
            "Price_Range": self.build_price_range(digikey_price, mouser_price),
        }

    def _digikey_variations(self, item: dict) -> list[dict]:
        # Keyword endpoint usually exposes "ProductVariations", while some schemas
        # use singular "ProductVariation"; detail endpoint may wrap under "Product".
        variations = item.get("ProductVariations") or item.get("ProductVariation") or []
        if isinstance(variations, list):
            return [v for v in variations if isinstance(v, dict)]
        return []

    def _digikey_part_number(self, item: dict) -> str:
        direct = item.get("DigiKeyPartNumber") or item.get("DigiKeyProductNumber")
        if isinstance(direct, str) and direct.strip():
            return direct.strip()
        for var in self._digikey_variations(item):
            pn = var.get("DigiKeyProductNumber") or var.get("DigiKeyPartNumber")
            if isinstance(pn, str) and pn.strip():
                return pn.strip()
        return ""

    def _digikey_price(self, item: dict) -> str:
        unit_price = self._safe_float(item.get("UnitPrice"))
        if unit_price is not None:
            return self._format_usd(unit_price)
        for var in self._digikey_variations(item):
            for bucket_name in ("MyPricing", "StandardPricing"):
                buckets = var.get(bucket_name)
                if not isinstance(buckets, list) or not buckets:
                    continue
                first = buckets[0] if isinstance(buckets[0], dict) else {}
                price = self._safe_float(first.get("UnitPrice"))
                if price is not None:
                    return self._format_usd(price)
        return ""

    def _supplier_part_from_digikey_item(self, item: dict) -> SupplierPart | None:
        if not isinstance(item, dict):
            return None
        mfg = item.get("Manufacturer") or {}
        mfg_name = ""
        if isinstance(mfg, dict):
            mfg_name = mfg.get("Value") or mfg.get("Name") or ""
        desc = item.get("Description") or {}
        product_desc = desc.get("ProductDescription", "") if isinstance(desc, dict) else str(desc)
        return SupplierPart(
            source="DigiKey",
            mpn=item.get("ManufacturerProductNumber", ""),
            manufacturer=mfg_name,
            description=product_desc,
            datasheet=item.get("PrimaryDatasheet", "") or item.get("DatasheetUrl", ""),
            digikey_pn=self._digikey_part_number(item),
            mouser_pn="",
            price=self._digikey_price(item),
            quantity_available=self._safe_int(item.get("QuantityAvailable", 0)),
            product_url=item.get("ProductUrl", ""),
        )

    def search_digikey_keyword(self, query: str, limit: int = 10) -> list[SupplierPart]:
        token = self._get_digikey_token()
        client_id = self.config.get("DIGIKEY_CLIENT_ID", "")
        if not token or not client_id:
            return []
        response = requests.post(
            "https://api.digikey.com/products/v4/search/keyword",
            headers={**self._digikey_headers(token, client_id), "Content-Type": "application/json"},
            json={"Keywords": query, "RecordCount": limit},
            timeout=20,
        )
        if response.status_code >= 400:
            return []
        payload = response.json()
        products = payload.get("Products", [])
        # Product detail API shape may return single "Product".
        if not products and isinstance(payload.get("Product"), dict):
            products = [payload.get("Product")]
        results: list[SupplierPart] = []
        for item in products:
            part = self._supplier_part_from_digikey_item(item)
            if part:
                results.append(part)
        return results

    def search_digikey_product_pricing(self, product_number: str, limit: int = 10) -> list[SupplierPart]:
        query_text = self._normalize_query(product_number)
        if not query_text:
            return []
        token = self._get_digikey_token()
        client_id = self.config.get("DIGIKEY_CLIENT_ID", "")
        if not token or not client_id:
            return []
        encoded = quote(query_text, safe="")
        try:
            response = requests.get(
                f"https://api.digikey.com/products/v4/search/{encoded}/pricing",
                params={"limit": max(1, min(int(limit), 10)), "offset": 0},
                headers=self._digikey_headers(token, client_id),
                timeout=20,
            )
        except requests.exceptions.RequestException as exc:
            self._log_event(
                "digikey",
                "product_pricing_request_error",
                {"query": query_text, "error": str(exc)},
            )
            return []
        if response.status_code >= 400:
            self._log_event(
                "digikey",
                "product_pricing_http_error",
                {"query": query_text, "status_code": response.status_code},
            )
            return []
        try:
            payload = response.json()
        except ValueError:
            self._log_event("digikey", "product_pricing_json_decode_error", {"query": query_text})
            return []
        products = payload.get("ProductPricings") or payload.get("Products") or []
        results: list[SupplierPart] = []
        for item in products:
            part = self._supplier_part_from_digikey_item(item)
            if part:
                results.append(part)
        return results

    def search_digikey_product_details(self, product_number: str, limit: int = 10) -> list[SupplierPart]:
        del limit  # Signature compatibility with SearchWorker providers.
        query_text = self._normalize_query(product_number)
        if not query_text:
            return []
        token = self._get_digikey_token()
        client_id = self.config.get("DIGIKEY_CLIENT_ID", "")
        if not token or not client_id:
            self._log_event("digikey", "product_details_missing_credentials", {"query": query_text})
            return []
        encoded = quote(query_text, safe="")
        url = f"https://api.digikey.com/products/v4/search/{encoded}/productdetails"
        try:
            response = requests.get(
                url,
                headers=self._digikey_headers(token, client_id),
                timeout=20,
            )
        except requests.exceptions.RequestException as exc:
            self._log_event(
                "digikey",
                "product_details_request_error",
                {"query": query_text, "error": str(exc)},
            )
            return []
        if response.status_code >= 400:
            self._log_event(
                "digikey",
                "product_details_http_error",
                {"query": query_text, "status_code": response.status_code},
            )
            return []
        try:
            payload = response.json()
        except ValueError:
            self._log_event("digikey", "product_details_json_decode_error", {"query": query_text})
            return []
        product = payload.get("Product")
        part = self._supplier_part_from_digikey_item(product)
        results = [part] if part else []
        self._log_event(
            "digikey",
            "product_details_result_count",
            {"query": query_text, "count": len(results)},
        )
        return results

    def search_mouser_keyword(self, query: str, limit: int = 10) -> list[SupplierPart]:
        query_text = self._normalize_query(query)
        if not query_text:
            self._log_event("mouser", "skip_empty_query", {"mode": "keyword"})
            return []
        payload = self._mouser_post(
            "search/keyword",
            {
                "SearchByKeywordRequest": {
                    "keyword": query_text,
                    "records": self._mouser_records(limit),
                    "startingRecord": 0,
                    # Doc supports string names or integer IDs; prefer string readability.
                    "searchOptions": "None",
                    "searchWithYourSignUpLanguage": "false",
                    "mouserPaysCustomsAndDuties": False,
                }
            },
        )
        results = self._mouser_results_from_payload(payload)
        self._log_event(
            "mouser",
            "keyword_result_count",
            {"query": query_text, "limit": limit, "count": len(results)},
        )
        return results

    def search_mouser_partnumber(self, query: str, limit: int = 10) -> list[SupplierPart]:
        query_text = self._normalize_query(query)
        if not query_text:
            self._log_event("mouser", "skip_empty_query", {"mode": "partnumber"})
            return []
        # Try exact first, then relaxed part-number search.
        for option in ("Exact", "None"):
            payload = self._mouser_post(
                "search/partnumber",
                {
                    "SearchByPartRequest": {
                        "mouserPartNumber": query_text,
                        "partSearchOptions": option,
                    }
                },
            )
            results = self._mouser_results_from_payload(payload)
            self._log_event(
                "mouser",
                "partnumber_result_count",
                {"query": query_text, "limit": limit, "partSearchOptions": option, "count": len(results)},
            )
            if results:
                return results[: self._mouser_records(limit)]
        return []

    def search_mouser(self, query: str, limit: int = 10) -> list[SupplierPart]:
        mouser_part = self.search_mouser_partnumber(query, limit)
        mouser_kw = self.search_mouser_keyword(query, limit)
        return self._dedupe_supplier_parts(mouser_part + mouser_kw)

    def search_by_mpn(self, mpn: str, limit: int = 10) -> list[SupplierPart]:
        query_text = self._normalize_query(mpn)
        if not query_text:
            return []

        def digikey_lookup() -> list[SupplierPart]:
            results = self.search_digikey_product_details(query_text, limit)
            if results:
                return results
            pricing_results = self.search_digikey_product_pricing(query_text, limit)
            normalized = self._normalized_pn(query_text)
            if not normalized:
                return pricing_results
            exact = [part for part in pricing_results if self._normalized_pn(part.mpn) == normalized]
            return exact or pricing_results

        with ThreadPoolExecutor(max_workers=2) as executor:
            digikey_future = executor.submit(digikey_lookup)
            mouser_future = executor.submit(self.search_mouser_partnumber, query_text, limit)
            try:
                digikey = digikey_future.result()
            except Exception:
                self._log_event("digikey", "search_by_mpn_exception", {"query": query_text})
                digikey = []
            try:
                mouser = mouser_future.result()
            except Exception:
                mouser = []
        results = self._dedupe_supplier_parts(digikey + mouser)
        self._log_event(
            "supplier",
            "search_by_mpn_counts",
            {
                "query": query_text,
                "limit": limit,
                "digikey_count": len(digikey),
                "mouser_count": len(mouser),
            },
        )
        return results

    def resolve_supplier_pns(self, mpn: str, limit: int = 10) -> dict[str, str]:
        query_text = self._normalize_query(mpn)
        if not query_text:
            return {
                "DigiKey_PN": "",
                "Mouser_PN": "",
                "DigiKey_Price": "",
                "Mouser_Price": "",
                "Price_Range": "?",
            }
        resolved = self.search_by_mpn(query_text, limit=limit)
        normalized_mpn = self._normalized_pn(query_text)
        digikey_parts = [p for p in resolved if p.source.lower().startswith("digikey")]
        mouser_parts = [p for p in resolved if p.source.lower().startswith("mouser")]
        if normalized_mpn:
            digikey_exact = [p for p in digikey_parts if self._normalized_pn(p.mpn) == normalized_mpn]
            mouser_exact = [p for p in mouser_parts if self._normalized_pn(p.mpn) == normalized_mpn]
        else:
            digikey_exact = digikey_parts
            mouser_exact = mouser_parts
        digikey_pick = self._pick_best_part(digikey_exact or digikey_parts, query_text, mpn_hint=query_text)
        mouser_pick = self._pick_best_part(mouser_exact or mouser_parts, query_text, mpn_hint=query_text)
        mouser_pn = self._clean_supplier_pn(mouser_pick.mouser_pn if mouser_pick else "")
        if not mouser_pn and mouser_pick:
            mouser_pn = self._clean_supplier_pn(mouser_pick.mpn)
        digikey_price = self._clean_price_text(digikey_pick.price if digikey_pick else "")
        mouser_price = self._clean_price_text(mouser_pick.price if mouser_pick else "")
        return {
            "DigiKey_PN": (digikey_pick.digikey_pn if digikey_pick else "").strip(),
            "Mouser_PN": mouser_pn,
            "DigiKey_Price": digikey_price,
            "Mouser_Price": mouser_price,
            "Price_Range": self.build_price_range(digikey_price, mouser_price),
        }

    def search_all(self, query: str, limit: int = 10) -> list[SupplierPart]:
        # Run supplier searches concurrently so one API does not block the other.
        with ThreadPoolExecutor(max_workers=2) as executor:
            digikey_future = executor.submit(self.search_digikey_keyword, query, limit)
            mouser_future = executor.submit(self.search_mouser, query, limit)
            try:
                digikey = digikey_future.result()
            except Exception:
                digikey = []
            try:
                mouser = mouser_future.result()
            except Exception:
                mouser = []
        self._log_event(
            "supplier",
            "search_all_counts",
            {
                "query": query,
                "limit": limit,
                "digikey_count": len(digikey),
                "mouser_count": len(mouser),
            },
        )
        return digikey + mouser

