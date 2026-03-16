from __future__ import annotations

import subprocess
from dataclasses import dataclass
from pathlib import Path
import re

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


DEFAULT_SUBMODULE_PATHS = (
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


def _declared_submodule_paths(repo_root: Path) -> set[str]:
    gitmodules = repo_root / ".gitmodules"
    if not gitmodules.exists():
        return set()
    try:
        text = gitmodules.read_text(encoding="utf-8", errors="ignore")
    except OSError:
        return set()
    out: set[str] = set()
    for line in text.splitlines():
        match = re.match(r"^\s*path\s*=\s*(.+?)\s*$", line)
        if match:
            out.add(match.group(1).replace("\\", "/"))
    return out


def _effective_submodule_paths(repo_root: Path, submodule_paths: list[str] | tuple[str, ...] | None) -> list[str]:
    requested = [*tuple(submodule_paths or DEFAULT_SUBMODULE_PATHS)]
    declared = _declared_submodule_paths(repo_root)
    if not declared:
        return requested
    return [path for path in requested if path in declared]


def submodules_ready(repo_root: Path, submodule_paths: list[str] | tuple[str, ...] | None = None) -> bool:
    paths = tuple(submodule_paths or DEFAULT_SUBMODULE_PATHS)
    return all((repo_root / rel / ".git").exists() or (repo_root / rel).exists() for rel in paths)


def ensure_submodules(repo_root: Path, submodule_paths: list[str] | tuple[str, ...] | None = None) -> SubmoduleResult:
    paths = _effective_submodule_paths(repo_root, submodule_paths)
    args = ["submodule", "update", "--init", "--depth", "1"]
    if paths:
        args.extend(paths)
    return _run_git(repo_root, args)


def update_submodules(repo_root: Path, submodule_paths: list[str] | tuple[str, ...] | None = None) -> SubmoduleResult:
    paths = _effective_submodule_paths(repo_root, submodule_paths)
    args = ["submodule", "update", "--remote", "--depth", "1"]
    if paths:
        args.extend(paths)
    return _run_git(
        repo_root,
        args,
    )


def submodule_heads(repo_root: Path, submodule_paths: list[str] | tuple[str, ...] | None = None) -> dict[str, str]:
    paths = tuple(_effective_submodule_paths(repo_root, submodule_paths))
    heads: dict[str, str] = {}
    for rel in paths:
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

    def __init__(self, repo_root: Path, mode: str, submodule_paths: list[str] | tuple[str, ...] | None = None):
        super().__init__()
        self.repo_root = repo_root
        self.mode = mode
        self.submodule_paths = tuple(submodule_paths or DEFAULT_SUBMODULE_PATHS)

    def run(self) -> None:  # pragma: no cover - exercised through UI
        if self.mode == "ensure":
            result = ensure_submodules(self.repo_root, self.submodule_paths)
        else:
            result = update_submodules(self.repo_root, self.submodule_paths)
        self.completed.emit(result.ok, result.output)
