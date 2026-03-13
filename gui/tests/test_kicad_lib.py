from __future__ import annotations

from pathlib import Path

from app.kicad_lib import KiCadLibraryIndex, index_footprint_libraries, index_symbol_libraries


def test_index_symbol_libraries_extracts_top_level_names(tmp_path: Path) -> None:
    symbols_dir = tmp_path / "symbols"
    symbols_dir.mkdir()
    (symbols_dir / "Device.kicad_sym").write_text(
        '(kicad_symbol_lib (version 20230121)\n'
        '  (symbol "Device:R")\n'
        '  (symbol "Device:R_1_1")\n'
        '  (symbol "Device:C")\n'
        ')\n',
        encoding="utf-8",
    )
    entries = index_symbol_libraries(symbols_dir, source="local")
    names = sorted(e.name for e in entries)
    assert names == ["Device:C", "Device:R"]


def test_index_footprint_libraries_reads_pretty_dirs(tmp_path: Path) -> None:
    footprints = tmp_path / "footprints" / "Package_SO.pretty"
    footprints.mkdir(parents=True)
    (footprints / "SOIC-8.kicad_mod").write_text("(footprint SOIC-8)", encoding="utf-8")
    entries = index_footprint_libraries(tmp_path / "footprints", source="kicad")
    assert len(entries) == 1
    assert entries[0].name == "Package_SO:SOIC-8"
    assert entries[0].source == "kicad"


def test_index_dedupes_local_over_kicad(tmp_path: Path) -> None:
    # Local symbol and KiCad symbol with same name should resolve to local.
    (tmp_path / "symbols").mkdir()
    (tmp_path / "symbols" / "Device.kicad_sym").write_text(
        '(kicad_symbol_lib (version 20230121) (symbol "Device:R"))',
        encoding="utf-8",
    )
    (tmp_path / "libs" / "kicad-symbols").mkdir(parents=True)
    (tmp_path / "libs" / "kicad-symbols" / "Device.kicad_sym").write_text(
        '(kicad_symbol_lib (version 20230121) (symbol "Device:R"))',
        encoding="utf-8",
    )
    (tmp_path / "footprints").mkdir()
    (tmp_path / "libs" / "kicad-footprints").mkdir(parents=True)

    index = KiCadLibraryIndex(tmp_path)
    index.rebuild()
    resolved = index.resolve("Device:R", "symbol")
    assert resolved is not None
    assert resolved.source == "local"


def test_search_prefers_prefix_matches(tmp_path: Path) -> None:
    (tmp_path / "symbols").mkdir()
    (tmp_path / "symbols" / "Test.kicad_sym").write_text(
        '(kicad_symbol_lib (version 20230121) (symbol "Test:LM358") (symbol "Test:OPA990"))',
        encoding="utf-8",
    )
    (tmp_path / "footprints").mkdir()
    (tmp_path / "libs" / "kicad-symbols").mkdir(parents=True)
    (tmp_path / "libs" / "kicad-footprints").mkdir(parents=True)

    index = KiCadLibraryIndex(tmp_path)
    index.rebuild()
    results = index.search("Test:LM", "symbol")
    assert results
    assert results[0].name == "Test:LM358"
