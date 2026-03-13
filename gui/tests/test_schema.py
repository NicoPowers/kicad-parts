"""Tests for schema discovery and helpers in schema.py."""
from __future__ import annotations

from pathlib import Path

import pytest

from app.schema import (
    CategorySchema,
    category_from_filename,
    default_row_for_headers,
    discover_category_schemas,
    infer_column_type,
)


class TestCategoryFromFilename:
    def test_normal(self):
        assert category_from_filename(Path("g-res.csv")) == "res"

    def test_cap(self):
        assert category_from_filename(Path("g-cap.csv")) == "cap"

    def test_long_name(self):
        assert category_from_filename(Path("g-custom.csv")) == "custom"


class TestInferColumnType:
    def test_url_column(self):
        assert infer_column_type("Datasheet") == "url"
        assert infer_column_type("LCSC") == "url"
        assert infer_column_type("DigiKey_PN") == "url"
        assert infer_column_type("Mouser_PN") == "url"

    def test_number_column(self):
        assert infer_column_type("Value") == "number"
        assert infer_column_type("Voltage") == "number"

    def test_text_column(self):
        assert infer_column_type("Manufacturer") == "text"
        assert infer_column_type("Description") == "text"


class TestDefaultRowForHeaders:
    def test_all_empty(self):
        row = default_row_for_headers(["IPN", "Description", "Value"])
        assert row == {"IPN": "", "Description": "", "Value": ""}

    def test_fixed_fields_blank(self):
        row = default_row_for_headers(["Symbol", "Footprint", "Manufacturer"])
        assert all(v == "" for v in row.values())


class TestDiscoverCategorySchemas:
    def test_finds_csvs(self, tmp_path: Path):
        (tmp_path / "g-res.csv").write_text("IPN,Description\n", encoding="utf-8")
        (tmp_path / "g-cap.csv").write_text("IPN,Description,Capacitance\n", encoding="utf-8")
        schemas = discover_category_schemas(tmp_path)
        assert len(schemas) == 2
        keys = {s.key for s in schemas}
        assert keys == {"res", "cap"}

    def test_ignores_non_g_files(self, tmp_path: Path):
        (tmp_path / "substitutes.csv").write_text("IPN,MPN\n", encoding="utf-8")
        schemas = discover_category_schemas(tmp_path)
        assert schemas == []

    def test_schema_headers(self, tmp_path: Path):
        (tmp_path / "g-tst.csv").write_text("IPN,Description,Foo\n", encoding="utf-8")
        schemas = discover_category_schemas(tmp_path)
        assert schemas[0].headers == ["IPN", "Description", "Foo"]

    def test_required_columns(self, tmp_path: Path):
        (tmp_path / "g-tst.csv").write_text("IPN,Description,Symbol,Footprint,Value\n", encoding="utf-8")
        schemas = discover_category_schemas(tmp_path)
        assert set(schemas[0].required_columns) == {"IPN", "Description", "Symbol", "Footprint"}
