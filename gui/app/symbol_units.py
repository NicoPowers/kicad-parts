from __future__ import annotations


def item_matches_variant(item: object, selected_unit: int, selected_demorgan: int) -> bool:
    unit = int(getattr(item, "unit", 0) or 0)
    demorgan = int(getattr(item, "demorgan", 0) or 0)
    return unit in {0, selected_unit} and demorgan in {0, selected_demorgan}


def unit_options_from_symbol(symbol: object) -> list[tuple[int, str]]:
    unit_count = max(1, int(getattr(symbol, "unit_count", 0) or 0))
    unit_names = getattr(symbol, "unit_names", {}) or {}
    options: list[tuple[int, str]] = []
    for unit in range(1, unit_count + 1):
        name = unit_names.get(unit)
        label = f"Unit {unit}" if not name else f"Unit {unit}: {name}"
        options.append((unit, label))
    return options
