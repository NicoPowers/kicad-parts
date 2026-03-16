from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

import pytest

from app import provider_sync


def test_onboarding_stage_sequence_success(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    progress: list[str] = []

    def fake_run(cmd, cwd, capture_output, text, check):
        target = cmd[-1]
        if cmd[1] == "clone":
            return SimpleNamespace(returncode=0, stdout="cloned", stderr="")
        if target.startswith("git@"):
            return SimpleNamespace(returncode=1, stdout="", stderr="ssh failed")
        return SimpleNamespace(returncode=0, stdout="ok", stderr="")

    monkeypatch.setattr("app.provider_sync.subprocess.run", fake_run)

    probe = provider_sync.probe_repo_access("https://github.com/org/repo.git", progress_cb=progress.append)
    assert probe.ok is True

    checkout_dir, _ = provider_sync.ensure_provider_checkout(
        tmp_path,
        "client-a",
        probe.effective_url,
        progress_cb=progress.append,
    )
    checkout_dir.mkdir(parents=True, exist_ok=True)
    (checkout_dir / "3D Models").mkdir(parents=True, exist_ok=True)
    (checkout_dir / "3D Models" / "part.step").write_bytes(b"step")
    suggestion = provider_sync.suggest_library_mapping(checkout_dir)
    assert suggestion.models3d.selected == "3D Models"

    assert progress[:3] == [
        "Validating SSH access...",
        "Trying HTTPS fallback...",
        "Cloning provider repository...",
    ]


def test_onboarding_probe_failure_skips_checkout(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    progress: list[str] = []
    checkout_called = False

    def fake_run(cmd, cwd, capture_output, text, check):
        return SimpleNamespace(returncode=1, stdout="", stderr="fatal")

    monkeypatch.setattr("app.provider_sync.subprocess.run", fake_run)
    probe = provider_sync.probe_repo_access("https://github.com/org/repo.git", progress_cb=progress.append)
    assert probe.ok is False

    if probe.ok:
        checkout_called = True
        provider_sync.ensure_provider_checkout(tmp_path, "client-a", probe.effective_url, progress_cb=progress.append)

    assert checkout_called is False
    assert progress[:2] == ["Validating SSH access...", "Trying HTTPS fallback..."]
