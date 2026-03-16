from __future__ import annotations

from pathlib import Path

from app.aggregate import merge_provider_csvs, rebuild_aggregate, rebuild_aggregate_links, validate_provider_csvs
from app.provider_config import Provider, ProviderRegistry


def _provider(tmp_path: Path, provider_id: str, prefix: str) -> Provider:
    base = tmp_path / "libs" / "providers" / provider_id
    (base / "Symbols").mkdir(parents=True, exist_ok=True)
    (base / "Footprints").mkdir(parents=True, exist_ok=True)
    (base / "3D Models").mkdir(parents=True, exist_ok=True)
    (base / "design_blocks").mkdir(parents=True, exist_ok=True)
    (base / "database").mkdir(parents=True, exist_ok=True)
    (base / "database" / "g-res.csv").write_text(
        "IPN,Description,Symbol,Footprint\nRES-0001-0001,Resistor,g-res:R_US,g-res:R_0603\n",
        encoding="utf-8",
    )
    return Provider(
        id=provider_id,
        display_name=provider_id,
        prefix=prefix,
        visibility="private",
        priority=500,
        symbols_path=Path(f"libs/providers/{provider_id}/Symbols"),
        footprints_path=Path(f"libs/providers/{provider_id}/Footprints"),
        models3d_path=Path(f"libs/providers/{provider_id}/3D Models"),
        design_blocks_path=Path(f"libs/providers/{provider_id}/design_blocks"),
        database_path=Path(f"libs/providers/{provider_id}/database"),
        repo_path=Path(f"libs/providers/{provider_id}"),
        source="provider",
    )


def test_validate_provider_csvs_success(tmp_path: Path) -> None:
    provider = _provider(tmp_path, "client-a", "CA")
    assert validate_provider_csvs(provider, tmp_path) == []


def test_merge_provider_csvs_prefixes_ipn_and_refs(tmp_path: Path) -> None:
    provider = _provider(tmp_path, "client-a", "CA")
    registry = ProviderRegistry(providers=[provider], source_name="test")
    warnings, stats = merge_provider_csvs(tmp_path, registry)
    assert warnings == []
    assert stats["rows_merged"] == 1
    out = (tmp_path / "database" / "g-res.csv").read_text(encoding="utf-8")
    assert "CA-RES-0001-0001" in out
    assert "CA-g-res:R_US" in out
    assert "CA-g-res:R_0603" in out


def test_rebuild_aggregate_links_creates_provider_dirs(tmp_path: Path) -> None:
    provider = _provider(tmp_path, "client-a", "CA")
    registry = ProviderRegistry(providers=[provider], source_name="test")
    warnings = rebuild_aggregate_links(tmp_path, registry)
    assert warnings == []
    assert (tmp_path / "symbols" / "CA").exists()
    assert (tmp_path / "footprints" / "CA").exists()
    assert (tmp_path / "3d-models" / "CA").exists()
    assert (tmp_path / "design-blocks" / "CA").exists()


def test_rebuild_aggregate_skips_missing_provider_checkout_with_warning(tmp_path: Path) -> None:
    present = _provider(tmp_path, "client-a", "CA")
    missing = Provider(
        id="client-b",
        display_name="client-b",
        prefix="CB",
        visibility="private",
        priority=400,
        symbols_path=Path("libs/providers/client-b/Symbols"),
        footprints_path=Path("libs/providers/client-b/Footprints"),
        models3d_path=Path("libs/providers/client-b/3D Models"),
        design_blocks_path=Path("libs/providers/client-b/design_blocks"),
        database_path=Path("libs/providers/client-b/database"),
        repo_path=Path("libs/providers/client-b"),
        source="provider",
    )
    registry = ProviderRegistry(providers=[present, missing], source_name="test")

    result = rebuild_aggregate(tmp_path, registry)

    assert result.ok
    assert any("client-b" in warning and "checkout missing" in warning for warning in result.warnings)
