from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

import pytest

from app import provider_sync


def test_probe_repo_access_falls_back_to_https(monkeypatch: pytest.MonkeyPatch) -> None:
    calls: list[list[str]] = []
    progress: list[str] = []

    def fake_run(cmd, cwd, capture_output, text, check):
        calls.append(cmd)
        target = cmd[-1]
        if target.startswith("git@"):
            return SimpleNamespace(returncode=1, stdout="", stderr="ssh failed")
        return SimpleNamespace(returncode=0, stdout="ok", stderr="")

    monkeypatch.setattr("app.provider_sync.subprocess.run", fake_run)
    result = provider_sync.probe_repo_access(
        "https://github.com/org/repo.git",
        progress_cb=lambda message: progress.append(message),
    )
    assert result.ok is True
    assert result.auth_method == "https"
    assert any(c[-1].startswith("git@") for c in calls)
    assert progress[:2] == ["Validating SSH access...", "Trying HTTPS fallback..."]


def test_suggest_library_mapping_prefers_named_folders(tmp_path: Path) -> None:
    repo = tmp_path / "provider"
    (repo / "symbols").mkdir(parents=True)
    (repo / "symbols" / "Device.kicad_sym").write_text('(kicad_symbol_lib (symbol "Device:R"))', encoding="utf-8")
    (repo / "footprints" / "Chip.pretty").mkdir(parents=True)
    (repo / "footprints" / "Chip.pretty" / "R_0603.kicad_mod").write_text("(footprint R_0603)", encoding="utf-8")
    (repo / "models3d" / "Resistor_SMD.3dshapes").mkdir(parents=True)
    (repo / "models3d" / "Resistor_SMD.3dshapes" / "R_0603.step").write_bytes(b"step")
    (repo / "design_blocks").mkdir(parents=True)
    (repo / "database").mkdir(parents=True)
    (repo / "database" / "g-res.csv").write_text("IPN,Description,Symbol,Footprint\nRES-0001-0001,a,b,c\n", encoding="utf-8")

    suggestion = provider_sync.suggest_library_mapping(repo)
    assert suggestion.symbols.selected == "symbols"
    assert suggestion.footprints.selected == "footprints/Chip.pretty"
    assert suggestion.models3d.selected.startswith("models3d")
    assert suggestion.design_blocks.selected == "design_blocks"
    assert suggestion.database.selected == "database"


def test_suggest_library_mapping_prefers_3d_root_folder(tmp_path: Path) -> None:
    repo = tmp_path / "provider"
    (repo / "3D Models" / "CAN").mkdir(parents=True)
    (repo / "3D Models" / "ADC").mkdir(parents=True)
    (repo / "3D Models" / "CAN" / "can.step").write_bytes(b"step")
    (repo / "3D Models" / "ADC" / "adc.step").write_bytes(b"step")

    suggestion = provider_sync.suggest_library_mapping(repo)
    assert suggestion.models3d.selected == "3D Models"


def test_suggest_library_mapping_prefers_shallow_semantic_models_root(tmp_path: Path) -> None:
    repo = tmp_path / "provider"
    (repo / "mechanical" / "models").mkdir(parents=True)
    (repo / "mechanical" / "models" / "ic.step").write_bytes(b"step")
    (repo / "mechanical" / "models" / "nested").mkdir(parents=True, exist_ok=True)
    (repo / "mechanical" / "models" / "nested" / "other.step").write_bytes(b"step2")

    suggestion = provider_sync.suggest_library_mapping(repo)
    assert suggestion.models3d.selected in {"mechanical/models", "mechanical"}


def test_ensure_provider_checkout_updates_existing_repo(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    checkout = tmp_path / "libs" / "providers" / "client-a"
    (checkout / ".git").mkdir(parents=True)
    seen: list[list[str]] = []
    progress: list[str] = []

    def fake_run(cmd, cwd, capture_output, text, check):
        seen.append(cmd)
        return SimpleNamespace(returncode=0, stdout="ok", stderr="")

    monkeypatch.setattr("app.provider_sync.subprocess.run", fake_run)
    path, message = provider_sync.ensure_provider_checkout(
        tmp_path,
        "client-a",
        "https://example.com/repo.git",
        progress_cb=lambda msg: progress.append(msg),
    )
    assert path == checkout
    assert "updated" in message
    assert any("fetch" in call for call in seen)
    assert progress and "Updating existing provider checkout..." in progress[0]
