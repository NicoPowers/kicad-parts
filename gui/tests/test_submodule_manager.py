from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

import pytest

from app import submodule_manager


def test_submodules_ready_false_for_missing_paths(tmp_path: Path) -> None:
    assert submodule_manager.submodules_ready(tmp_path) is False


def test_submodules_ready_true_when_dirs_exist(tmp_path: Path) -> None:
    for rel in submodule_manager.SUBMODULE_PATHS:
        (tmp_path / rel).mkdir(parents=True, exist_ok=True)
    assert submodule_manager.submodules_ready(tmp_path) is True


def test_ensure_submodules_invokes_git(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    calls: list[list[str]] = []

    def fake_run(cmd, cwd, capture_output, text, check):
        calls.append(cmd)
        return SimpleNamespace(returncode=0, stdout="ok", stderr="")

    monkeypatch.setattr("app.submodule_manager.subprocess.run", fake_run)
    result = submodule_manager.ensure_submodules(tmp_path)
    assert result.ok is True
    assert calls
    assert calls[0][0] == "git"
    assert "submodule" in calls[0]


def test_update_submodules_propagates_errors(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    def fake_run(cmd, cwd, capture_output, text, check):
        return SimpleNamespace(returncode=1, stdout="", stderr="fatal: failed")

    monkeypatch.setattr("app.submodule_manager.subprocess.run", fake_run)
    result = submodule_manager.update_submodules(tmp_path)
    assert result.ok is False
    assert "fatal" in result.output


def test_submodule_heads_reports_missing_and_sha(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    (tmp_path / "libs" / "kicad-symbols").mkdir(parents=True, exist_ok=True)

    def fake_run(cmd, cwd, capture_output, text, check):
        if "kicad-symbols" in str(cwd):
            return SimpleNamespace(returncode=0, stdout="abc123\n", stderr="")
        return SimpleNamespace(returncode=1, stdout="", stderr="fatal")

    monkeypatch.setattr("app.submodule_manager.subprocess.run", fake_run)
    heads = submodule_manager.submodule_heads(tmp_path)
    assert heads["libs/kicad-symbols"] == "abc123"
    assert heads["libs/kicad-footprints"] == "missing"
