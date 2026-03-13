"""Tests for SupplierApiClient – unit tests with mocked HTTP and live integration tests.

Unit tests verify parsing logic with canned API response payloads.
Integration tests (marked ``@pytest.mark.live``) hit the real DigiKey/Mouser
APIs using credentials from secrets.env and are skipped by default.
Run them explicitly with:  ``pytest -m live -s``
"""
from __future__ import annotations

import json
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

import pytest

from app.supplier_api import SupplierApiClient, SupplierPart

WORKSPACE = Path(__file__).resolve().parents[2]
SECRETS_PATH = WORKSPACE / "secrets.env"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_client() -> SupplierApiClient:
    """Build a client from the workspace secrets.env (used by live tests)."""
    return SupplierApiClient(SECRETS_PATH)


def _mock_client() -> SupplierApiClient:
    """Build a client that never touches the network (for unit tests)."""
    with patch("app.supplier_api.dotenv_values", return_value={}):
        client = SupplierApiClient(SECRETS_PATH)
    client.config = {
        "DIGIKEY_CLIENT_ID": "fake-id",
        "DIGIKEY_CLIENT_SECRET": "fake-secret",
    }
    client._digikey_token = "fake-token"
    client._digikey_expiry = 9e12
    return client


def _fake_response(status_code: int, payload: dict) -> MagicMock:
    resp = MagicMock()
    resp.status_code = status_code
    resp.text = json.dumps(payload)
    resp.json.return_value = payload
    return resp


# ===================================================================
# UNIT TESTS – DigiKey parsing
# ===================================================================

class TestDigiKeyProductDetailsParser:
    """Verify _supplier_part_from_digikey_item and product_details parsing."""

    SAMPLE_PRODUCT = {
        "ManufacturerProductNumber": "C0603C180J5GACTU",
        "Manufacturer": {"Value": "KEMET"},
        "Description": {"ProductDescription": "CAP CER 18PF 50V C0G/NP0 0603"},
        "PrimaryDatasheet": "https://example.com/ds.pdf",
        "QuantityAvailable": 5000,
        "ProductUrl": "https://www.digikey.com/en/products/detail/kemet/C0603C180J5GACTU/...",
        "ProductVariations": [
            {
                "DigiKeyProductNumber": "399-C0603C180J5GACTU-ND",
                "StandardPricing": [{"BreakQuantity": 1, "UnitPrice": 0.10}],
            }
        ],
    }

    def test_parse_single_product(self):
        client = _mock_client()
        part = client._supplier_part_from_digikey_item(self.SAMPLE_PRODUCT)
        assert part is not None
        assert part.mpn == "C0603C180J5GACTU"
        assert part.manufacturer == "KEMET"
        assert part.digikey_pn == "399-C0603C180J5GACTU-ND"
        assert part.source == "DigiKey"

    def test_parse_none_returns_none(self):
        client = _mock_client()
        assert client._supplier_part_from_digikey_item(None) is None

    def test_parse_empty_dict(self):
        client = _mock_client()
        part = client._supplier_part_from_digikey_item({})
        assert part is not None
        assert part.mpn == ""

    @patch("app.supplier_api.requests.get")
    def test_product_details_200_with_product(self, mock_get):
        client = _mock_client()
        mock_get.return_value = _fake_response(200, {"Product": self.SAMPLE_PRODUCT})
        results = client.search_digikey_product_details("C0603C180J5GACTU")
        assert len(results) == 1
        assert results[0].mpn == "C0603C180J5GACTU"

    @patch("app.supplier_api.requests.get")
    def test_product_details_404_returns_empty(self, mock_get):
        client = _mock_client()
        mock_get.return_value = _fake_response(404, {"title": "Not Found"})
        results = client.search_digikey_product_details("NONEXISTENT")
        assert results == []

    @patch("app.supplier_api.requests.get")
    def test_product_details_product_key_missing(self, mock_get):
        """If the 200 response has no 'Product' key, return empty."""
        client = _mock_client()
        mock_get.return_value = _fake_response(200, {"SomeOtherKey": {}})
        results = client.search_digikey_product_details("C0603C180J5GACTU")
        assert results == []


class TestDigiKeyProductPricingParser:
    """Verify ProductPricing response parsing uses the correct key."""

    SAMPLE_PRICING_RESPONSE = {
        "ProductPricings": [
            {
                "ManufacturerProductNumber": "C0603C180J5GACTU",
                "Manufacturer": {"Value": "KEMET"},
                "Description": {"ProductDescription": "CAP CER 18PF 50V C0G 0603"},
                "QuantityAvailable": 5000,
                "ProductUrl": "https://www.digikey.com/...",
                "ProductVariations": [
                    {
                        "DigiKeyProductNumber": "399-C0603C180J5GACTU-ND",
                        "StandardPricing": [{"BreakQuantity": 1, "UnitPrice": 0.10}],
                    }
                ],
            }
        ],
        "ProductsCount": 1,
    }

    @patch("app.supplier_api.requests.get")
    def test_pricing_parses_product_pricings_key(self, mock_get):
        """The pricing endpoint returns 'ProductPricings', not 'Products'."""
        client = _mock_client()
        mock_get.return_value = _fake_response(200, self.SAMPLE_PRICING_RESPONSE)
        results = client.search_digikey_product_pricing("C0603C180J5GACTU")
        assert len(results) == 1
        assert results[0].mpn == "C0603C180J5GACTU"

    @patch("app.supplier_api.requests.get")
    def test_pricing_with_wrong_key_returns_empty(self, mock_get):
        """Regression: if we only read 'Products', we get nothing."""
        client = _mock_client()
        payload = {"Products": [], "ProductPricings": self.SAMPLE_PRICING_RESPONSE["ProductPricings"]}
        mock_get.return_value = _fake_response(200, payload)
        results = client.search_digikey_product_pricing("C0603C180J5GACTU")
        assert len(results) == 1, "Should read from ProductPricings, not Products"

    @patch("app.supplier_api.requests.get")
    def test_pricing_404(self, mock_get):
        client = _mock_client()
        mock_get.return_value = _fake_response(404, {})
        results = client.search_digikey_product_pricing("NONEXISTENT")
        assert results == []


# ===================================================================
# UNIT TESTS – Mouser parsing
# ===================================================================

class TestMouserPartNumberParser:
    SAMPLE_MOUSER_RESPONSE = {
        "Errors": [],
        "SearchResults": {
            "NumberOfResult": 1,
            "Parts": [
                {
                    "ManufacturerPartNumber": "C0603C180J5GACTU",
                    "Manufacturer": "KEMET",
                    "Description": "Multilayer Ceramic Capacitors MLCC - SMD/SMT 50V 18pF C0G 0603 5%",
                    "DataSheetUrl": "https://example.com/ds.pdf",
                    "MouserPartNumber": "80-C0603C180J5G",
                    "PriceBreaks": [{"Quantity": 1, "Price": "$0.10", "Currency": "USD"}],
                    "AvailabilityInStock": "82327",
                    "ProductDetailUrl": "https://www.mouser.com/ProductDetail/...",
                }
            ],
        },
    }

    def test_parse_mouser_results(self):
        client = _mock_client()
        results = client._mouser_results_from_payload(self.SAMPLE_MOUSER_RESPONSE)
        assert len(results) == 1
        assert results[0].mpn == "C0603C180J5GACTU"
        assert results[0].mouser_pn == "80-C0603C180J5G"
        assert results[0].source == "Mouser"
        assert results[0].quantity_available == 82327

    def test_parse_empty_payload(self):
        client = _mock_client()
        results = client._mouser_results_from_payload({})
        assert results == []


# ===================================================================
# UNIT TESTS – search_by_mpn orchestration
# ===================================================================

class TestSearchByMpn:
    @patch("app.supplier_api.requests.get")
    @patch("app.supplier_api.requests.post")
    def test_search_by_mpn_returns_combined_results(self, mock_post, mock_get):
        client = _mock_client()
        client.config["MOUSER_SEARCH_API_KEY"] = "fake-mouser-key"

        dk_product = {
            "ManufacturerProductNumber": "C0603C180J5GACTU",
            "Manufacturer": {"Value": "KEMET"},
            "Description": {"ProductDescription": "CAP CER 18PF"},
            "ProductVariations": [{"DigiKeyProductNumber": "399-ND", "StandardPricing": [{"UnitPrice": 0.10}]}],
        }
        mock_get.return_value = _fake_response(200, {"Product": dk_product})

        mouser_resp = {
            "Errors": [],
            "SearchResults": {
                "Parts": [
                    {
                        "ManufacturerPartNumber": "C0603C180J5GACTU",
                        "Manufacturer": "KEMET",
                        "Description": "CAP 18PF",
                        "MouserPartNumber": "80-C0603C180J5G",
                        "PriceBreaks": [{"Quantity": 1, "Price": "$0.10"}],
                        "AvailabilityInStock": "100",
                        "ProductDetailUrl": "",
                    }
                ]
            },
        }
        mock_post.return_value = _fake_response(200, mouser_resp)

        results = client.search_by_mpn("C0603C180J5GACTU")
        sources = {p.source for p in results}
        assert "DigiKey" in sources
        assert "Mouser" in sources


# ===================================================================
# UNIT TESTS – resolve_supplier_pns
# ===================================================================

class TestResolveSupplierPNs:
    def test_empty_mpn_returns_defaults(self):
        client = _mock_client()
        result = client.resolve_supplier_pns("")
        assert result["DigiKey_PN"] == ""
        assert result["Mouser_PN"] == ""
        assert result["Price_Range"] == "?"


# ===================================================================
# UNIT TESTS – helper methods
# ===================================================================

class TestHelpers:
    def test_normalized_pn(self):
        assert SupplierApiClient._normalized_pn("C0603C180J5GACTU") == "c0603c180j5gactu"
        assert SupplierApiClient._normalized_pn("399-C0603C180J5GACTU-ND") == "399c0603c180j5gactund"

    def test_build_price_range_both(self):
        client = _mock_client()
        assert client.build_price_range("$0.10", "$0.12") == "$0.1 - $0.12"

    def test_build_price_range_same(self):
        client = _mock_client()
        assert client.build_price_range("$0.10", "$0.10") == "$0.1"

    def test_build_price_range_one_missing(self):
        client = _mock_client()
        assert client.build_price_range("$0.10", "") == "$0.1"

    def test_build_price_range_none(self):
        client = _mock_client()
        assert client.build_price_range("", "") == "?"

    def test_normalize_query(self):
        assert SupplierApiClient._normalize_query("  C0603C180J5GACTU  ") == "C0603C180J5GACTU"

    def test_dedupe_supplier_parts(self):
        client = _mock_client()
        p1 = SupplierPart("DigiKey", "MPN1", "Mfg", "Desc", "", "DK1", "", "$0.10")
        p2 = SupplierPart("DigiKey", "MPN1", "Mfg", "Desc", "", "DK1", "", "$0.10")
        assert len(client._dedupe_supplier_parts([p1, p2])) == 1


# ===================================================================
# LIVE INTEGRATION TESTS – require secrets.env credentials
# ===================================================================

def _has_secrets() -> bool:
    if not SECRETS_PATH.exists():
        return False
    from dotenv import dotenv_values
    cfg = dotenv_values(SECRETS_PATH)
    return bool(cfg.get("DIGIKEY_CLIENT_ID") and cfg.get("MOUSER_SEARCH_API_KEY"))


_skip_no_creds = pytest.mark.skipif(not _has_secrets(), reason="No API credentials in secrets.env")

def live(cls):
    """Combine @pytest.mark.live with the credential check."""
    cls = pytest.mark.live(cls)
    cls = _skip_no_creds(cls)
    return cls

TEST_MPNS = [
    "C0603C180J5GACTU",
    "CL10C100JB8NNNC",
    "BSS138",
    "RC0603FR-0710KL",
]


@live
class TestLiveDigiKeyProductDetails:
    """Hit the real DigiKey ProductDetails endpoint."""

    @pytest.fixture(scope="class")
    def client(self) -> SupplierApiClient:
        return _make_client()

    @pytest.mark.parametrize("mpn", TEST_MPNS)
    def test_product_details(self, client, mpn):
        results = client.search_digikey_product_details(mpn)
        print(f"\n[ProductDetails] {mpn}: {len(results)} result(s)")
        for r in results:
            print(f"  -> mpn={r.mpn}, dk_pn={r.digikey_pn}, price={r.price}")


@live
class TestLiveDigiKeyProductPricing:
    """Hit the real DigiKey ProductPricing endpoint."""

    @pytest.fixture(scope="class")
    def client(self) -> SupplierApiClient:
        return _make_client()

    @pytest.mark.parametrize("mpn", TEST_MPNS)
    def test_product_pricing(self, client, mpn):
        results = client.search_digikey_product_pricing(mpn)
        print(f"\n[ProductPricing] {mpn}: {len(results)} result(s)")
        for r in results:
            print(f"  -> mpn={r.mpn}, dk_pn={r.digikey_pn}, price={r.price}")


@live
class TestLiveDigiKeyKeyword:
    """Hit the real DigiKey KeywordSearch endpoint for comparison."""

    @pytest.fixture(scope="class")
    def client(self) -> SupplierApiClient:
        return _make_client()

    @pytest.mark.parametrize("mpn", TEST_MPNS)
    def test_keyword_search(self, client, mpn):
        results = client.search_digikey_keyword(mpn, limit=5)
        print(f"\n[KeywordSearch] {mpn}: {len(results)} result(s)")
        for r in results:
            print(f"  -> mpn={r.mpn}, dk_pn={r.digikey_pn}, price={r.price}")


@live
class TestLiveMouserPartNumber:
    """Hit the real Mouser partnumber search endpoint."""

    @pytest.fixture(scope="class")
    def client(self) -> SupplierApiClient:
        return _make_client()

    @pytest.mark.parametrize("mpn", TEST_MPNS)
    def test_mouser_partnumber(self, client, mpn):
        results = client.search_mouser_partnumber(mpn)
        print(f"\n[Mouser PartNumber] {mpn}: {len(results)} result(s)")
        for r in results:
            print(f"  -> mpn={r.mpn}, mouser_pn={r.mouser_pn}, price={r.price}")


@live
class TestLiveSearchByMpn:
    """Hit the combined search_by_mpn endpoint."""

    @pytest.fixture(scope="class")
    def client(self) -> SupplierApiClient:
        return _make_client()

    @pytest.mark.parametrize("mpn", TEST_MPNS)
    def test_search_by_mpn(self, client, mpn):
        results = client.search_by_mpn(mpn)
        print(f"\n[search_by_mpn] {mpn}: {len(results)} result(s)")
        dk = [r for r in results if r.source == "DigiKey"]
        mo = [r for r in results if r.source == "Mouser"]
        print(f"  DigiKey: {len(dk)}, Mouser: {len(mo)}")
        for r in results:
            print(f"  [{r.source}] mpn={r.mpn}, dk_pn={r.digikey_pn}, mouser_pn={r.mouser_pn}, price={r.price}")


@live
class TestLiveResolveSupplierPNs:
    """Hit the combined resolve_supplier_pns method."""

    @pytest.fixture(scope="class")
    def client(self) -> SupplierApiClient:
        return _make_client()

    @pytest.mark.parametrize("mpn", TEST_MPNS)
    def test_resolve(self, client, mpn):
        result = client.resolve_supplier_pns(mpn)
        print(f"\n[resolve_supplier_pns] {mpn}:")
        for k, v in result.items():
            print(f"  {k}: {v}")
