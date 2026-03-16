from __future__ import annotations

import json
from pathlib import Path

from app.kicad_lib import KiCadLibraryIndex, index_footprint_libraries, index_symbol_libraries


def _write_provider_config(tmp_path: Path) -> None:
    payload = {
        "version": 1,
        "providers": [
            {
                "id": "client-a",
                "display_name": "Client A",
                "prefix": "CA",
                "visibility": "private",
                "priority": 500,
                "repo_url": "",
                "repo_path": ".",
                "symbols_path": "symbols",
                "footprints_path": "footprints",
                "models3d_path": "3d-models",
                "design_blocks_path": "design-blocks",
                "database_path": "database",
                "source": "provider",
            },
            {
                "id": "kicad",
                "display_name": "KiCad Reference",
                "prefix": "",
                "visibility": "public",
                "priority": 100,
                "repo_url": "",
                "repo_path": "libs/kicad-symbols",
                "symbols_path": "libs/kicad-symbols",
                "footprints_path": "libs/kicad-footprints",
                "models3d_path": "libs/kicad-packages3D",
                "design_blocks_path": "",
                "database_path": "",
                "source": "kicad",
            },
        ],
    }
    (tmp_path / "library-providers.yaml").write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


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
    _write_provider_config(tmp_path)

    index = KiCadLibraryIndex(tmp_path)
    index.rebuild()
    resolved = index.resolve("CA-Device:R", "symbol")
    assert resolved is not None
    assert resolved.source == "provider"


def test_search_prefers_prefix_matches(tmp_path: Path) -> None:
    (tmp_path / "symbols").mkdir()
    (tmp_path / "symbols" / "Test.kicad_sym").write_text(
        '(kicad_symbol_lib (version 20230121) (symbol "Test:LM358") (symbol "Test:OPA990"))',
        encoding="utf-8",
    )
    (tmp_path / "footprints").mkdir()
    (tmp_path / "libs" / "kicad-symbols").mkdir(parents=True)
    (tmp_path / "libs" / "kicad-footprints").mkdir(parents=True)
    _write_provider_config(tmp_path)

    index = KiCadLibraryIndex(tmp_path)
    index.rebuild()
    results = index.search("CA-Test:LM", "symbol")
    assert results
    assert results[0].name == "CA-Test:LM358"
