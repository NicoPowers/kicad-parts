from __future__ import annotations

import subprocess
from dataclasses import dataclass
from pathlib import Path

try:
    from PyQt6.QtCore import QThread, pyqtSignal
except Exception:  # pragma: no cover - test fallback when PyQt6 isn't installed
    class QThread:  # type: ignore[override]
        def __init__(self, *args, **kwargs):
            pass

    class _Signal:
        def emit(self, *args, **kwargs):
            return None

    def pyqtSignal(*_args, **_kwargs):  # type: ignore
        return _Signal()


SUBMODULE_PATHS = (
    "libs/kicad-symbols",
    "libs/kicad-footprints",
    "libs/kicad-library-utils",
)


@dataclass(frozen=True)
class SubmoduleResult:
    ok: bool
    output: str


def _run_git(repo_root: Path, args: list[str]) -> SubmoduleResult:
    proc = subprocess.run(
        ["git", *args],
        cwd=repo_root,
        capture_output=True,
        text=True,
        check=False,
    )
    output = (proc.stdout or "") + (proc.stderr or "")
    return SubmoduleResult(ok=proc.returncode == 0, output=output.strip())


def submodules_ready(repo_root: Path) -> bool:
    return all((repo_root / rel / ".git").exists() or (repo_root / rel).exists() for rel in SUBMODULE_PATHS)


def ensure_submodules(repo_root: Path) -> SubmoduleResult:
    return _run_git(repo_root, ["submodule", "update", "--init", "--depth", "1", *SUBMODULE_PATHS])


def update_submodules(repo_root: Path) -> SubmoduleResult:
    return _run_git(
        repo_root,
        ["submodule", "update", "--remote", "--depth", "1", *SUBMODULE_PATHS],
    )


def submodule_heads(repo_root: Path) -> dict[str, str]:
    heads: dict[str, str] = {}
    for rel in SUBMODULE_PATHS:
        submodule_dir = repo_root / rel
        if not submodule_dir.exists():
            heads[rel] = "missing"
            continue
        proc = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            cwd=submodule_dir,
            capture_output=True,
            text=True,
            check=False,
        )
        heads[rel] = proc.stdout.strip() if proc.returncode == 0 else "unknown"
    return heads


class SubmoduleWorker(QThread):
    completed = pyqtSignal(bool, str)

    def __init__(self, repo_root: Path, mode: str):
        super().__init__()
        self.repo_root = repo_root
        self.mode = mode

    def run(self) -> None:  # pragma: no cover - exercised through UI
        if self.mode == "ensure":
            result = ensure_submodules(self.repo_root)
        else:
            result = update_submodules(self.repo_root)
        self.completed.emit(result.ok, result.output)
