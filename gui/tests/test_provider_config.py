from __future__ import annotations

import json
from pathlib import Path

from app.provider_config import (
    CONFIG_FILE,
    Provider,
    bootstrap_local_provider_config,
    config_path,
    load_provider_registry,
    save_provider_registry,
    write_example_config,
)


def test_load_provider_registry_falls_back_to_legacy_defaults(tmp_path: Path) -> None:
    registry = load_provider_registry(tmp_path)
    ids = [provider.id for provider in registry.providers]
    assert "kicad" in ids
    assert registry.source_name == "legacy-defaults"


def test_bootstrap_writes_local_config(tmp_path: Path) -> None:
    target = bootstrap_local_provider_config(tmp_path)
    assert target == tmp_path / CONFIG_FILE
    assert target.exists()
    payload = json.loads(target.read_text(encoding="utf-8"))
    assert payload["providers"]


def test_save_provider_registry_round_trip(tmp_path: Path) -> None:
    providers = [
        Provider(
            id="client-a",
            display_name="Client A",
            prefix="CA",
            visibility="private",
            priority=500,
            symbols_path=Path("libs/providers/client-a/symbols"),
            footprints_path=Path("libs/providers/client-a/footprints"),
            models3d_path=Path("libs/providers/client-a/3d-models"),
            design_blocks_path=Path("libs/providers/client-a/design_blocks"),
            database_path=Path("libs/providers/client-a/database"),
            repo_url="git@github.com:org/client-a-libs.git",
            repo_path=Path("libs/providers/client-a"),
            auth_method_last_success="ssh",
            source="provider",
        ),
    ]
    save_provider_registry(tmp_path, providers)
    loaded = load_provider_registry(tmp_path)
    loaded_client = loaded.by_id("client-a")
    assert loaded_client is not None
    assert loaded_client.repo_url.endswith("client-a-libs.git")
    assert loaded_client.auth_method_last_success == "ssh"
    assert loaded_client.prefix == "CA"


def test_write_example_config_creates_tracked_template(tmp_path: Path) -> None:
    path = write_example_config(tmp_path)
    assert path.exists()
    assert "providers" in path.read_text(encoding="utf-8")
    assert config_path(tmp_path).exists() is False
