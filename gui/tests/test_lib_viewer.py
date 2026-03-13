from __future__ import annotations

from types import SimpleNamespace

from app.symbol_units import item_matches_variant, unit_options_from_symbol


def test_item_matches_variant_includes_shared_unit_and_demorgan() -> None:
    shared = SimpleNamespace(unit=0, demorgan=0)
    assert item_matches_variant(shared, selected_unit=2, selected_demorgan=1)


def test_item_matches_variant_rejects_other_unit() -> None:
    other_unit = SimpleNamespace(unit=3, demorgan=1)
    assert not item_matches_variant(other_unit, selected_unit=2, selected_demorgan=1)


def test_item_matches_variant_rejects_other_demorgan() -> None:
    other_style = SimpleNamespace(unit=2, demorgan=2)
    assert not item_matches_variant(other_style, selected_unit=2, selected_demorgan=1)


def test_unit_options_use_unit_names_when_present() -> None:
    symbol = SimpleNamespace(unit_count=3, unit_names={1: "A", 2: None, 3: "Power"})
    assert unit_options_from_symbol(symbol) == [
        (1, "Unit 1: A"),
        (2, "Unit 2"),
        (3, "Unit 3: Power"),
    ]


def test_unit_options_default_to_single_unit() -> None:
    symbol = SimpleNamespace(unit_count=0, unit_names={})
    assert unit_options_from_symbol(symbol) == [(1, "Unit 1")]
