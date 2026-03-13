from __future__ import annotations

import re
import time
from dataclasses import dataclass
from pathlib import Path

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

    def search_digikey_keyword(self, query: str, limit: int = 10) -> list[SupplierPart]:
        token = self._get_digikey_token()
        client_id = self.config.get("DIGIKEY_CLIENT_ID", "")
        if not token or not client_id:
            return []
        response = requests.post(
            "https://api.digikey.com/products/v4/search/keyword",
            headers={
                "Authorization": f"Bearer {token}",
                "X-DIGIKEY-Client-Id": client_id,
                "Content-Type": "application/json",
                "Accept": "application/json",
            },
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
            if not isinstance(item, dict):
                continue
            mfg = item.get("Manufacturer") or {}
            mfg_name = ""
            if isinstance(mfg, dict):
                mfg_name = mfg.get("Value") or mfg.get("Name") or ""
            desc = item.get("Description") or {}
            product_desc = desc.get("ProductDescription", "") if isinstance(desc, dict) else str(desc)
            results.append(
                SupplierPart(
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
            )
        return results

    def search_mouser_keyword(self, query: str, limit: int = 10) -> list[SupplierPart]:
        api_key = self.config.get("MOUSER_SEARCH_API_KEY", "")
        if not api_key:
            return []
        response = requests.post(
            f"https://api.mouser.com/api/v1.0/search/keyword?apiKey={api_key}",
            json={
                "SearchByKeywordRequest": {
                    "keyword": query,
                    "records": limit,
                    "startingRecord": 0,
                    "searchOptions": "None",
                    "searchWithYourSignUpLanguage": "false",
                }
            },
            timeout=20,
        )
        if response.status_code >= 400:
            return []
        parts = (((response.json().get("SearchResults") or {}).get("Parts")) or [])
        results: list[SupplierPart] = []
        for item in parts:
            results.append(
                SupplierPart(
                    source="Mouser",
                    mpn=item.get("ManufacturerPartNumber", ""),
                    manufacturer=item.get("Manufacturer", ""),
                    description=item.get("Description", ""),
                    datasheet=item.get("DataSheetUrl", ""),
                    digikey_pn="",
                    mouser_pn=item.get("MouserPartNumber", ""),
                    price=item.get("PriceBreaks", [{}])[0].get("Price", "") if item.get("PriceBreaks") else "",
                    quantity_available=self._safe_int(item.get("AvailabilityInStock", 0))
                    or self._parse_quantity_from_text(item.get("Availability", "")),
                    product_url=item.get("ProductDetailUrl", ""),
                )
            )
        return results

    def search_all(self, query: str, limit: int = 10) -> list[SupplierPart]:
        return self.search_digikey_keyword(query, limit) + self.search_mouser_keyword(query, limit)

