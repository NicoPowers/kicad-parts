from __future__ import annotations

import re
import subprocess
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import urlparse


@dataclass(frozen=True)
class GitProbeResult:
    ok: bool
    auth_method: str
    effective_url: str
    output: str


@dataclass(frozen=True)
class FolderSuggestion:
    selected: str
    candidates: list[str]
    selected_score: int = 0
    low_confidence: bool = False


@dataclass(frozen=True)
class MappingSuggestion:
    symbols: FolderSuggestion
    footprints: FolderSuggestion
    models3d: FolderSuggestion
    design_blocks: FolderSuggestion
    database: FolderSuggestion


def sanitize_provider_id(text: str) -> str:
    cleaned = re.sub(r"[^a-z0-9]+", "-", text.strip().lower()).strip("-")
    return cleaned or "provider"


def _run_git(args: list[str], cwd: Path | None = None) -> tuple[int, str]:
    proc = subprocess.run(
        ["git", *args],
        cwd=cwd,
        capture_output=True,
        text=True,
        check=False,
    )
    output = ((proc.stdout or "") + "\n" + (proc.stderr or "")).strip()
    return proc.returncode, output


def _https_to_ssh(url: str) -> str:
    parsed = urlparse(url)
    if not parsed.scheme.startswith("http"):
        return url
    host = parsed.netloc
    path = parsed.path.strip("/")
    if path.endswith(".git"):
        return f"git@{host}:{path}"
    return f"git@{host}:{path}.git"


def _ssh_to_https(url: str) -> str:
    # Converts git@host:owner/repo.git -> https://host/owner/repo.git
    m = re.match(r"^git@([^:]+):(.+)$", url.strip())
    if not m:
        return url
    host, path = m.groups()
    return f"https://{host}/{path}"


def probe_repo_access(repo_url: str, progress_cb: Callable[[str], None] | None = None) -> GitProbeResult:
    repo_url = repo_url.strip()
    if not repo_url:
        return GitProbeResult(False, "", "", "Repository URL is empty.")

    ssh_url = _https_to_ssh(repo_url)
    if progress_cb is not None:
        progress_cb("Validating SSH access...")
    code, output = _run_git(["ls-remote", ssh_url])
    if code == 0:
        return GitProbeResult(True, "ssh", ssh_url, output)

    https_url = _ssh_to_https(repo_url)
    if progress_cb is not None:
        progress_cb("Trying HTTPS fallback...")
    code_https, output_https = _run_git(["ls-remote", https_url])
    if code_https == 0:
        return GitProbeResult(True, "https", https_url, output_https)
    merged_output = (output + "\n\n" + output_https).strip()
    return GitProbeResult(False, "", "", merged_output or "Unable to connect to repository via SSH or HTTPS.")


def ensure_provider_checkout(
    workspace_root: Path,
    provider_id: str,
    repo_url: str,
    progress_cb: Callable[[str], None] | None = None,
) -> tuple[Path, str]:
    checkout_dir = workspace_root / "libs" / "providers" / sanitize_provider_id(provider_id)
    checkout_dir.parent.mkdir(parents=True, exist_ok=True)
    if (checkout_dir / ".git").exists():
        if progress_cb is not None:
            progress_cb("Updating existing provider checkout...")
        _run_git(["fetch", "--all", "--prune"], cwd=checkout_dir)
        _run_git(["pull", "--ff-only"], cwd=checkout_dir)
        return checkout_dir, "updated existing checkout"
    if progress_cb is not None:
        progress_cb("Cloning provider repository...")
    code, output = _run_git(["clone", "--depth", "1", repo_url, str(checkout_dir)], cwd=workspace_root)
    if code != 0:
        raise RuntimeError(output or f"git clone failed for {repo_url}")
    return checkout_dir, "cloned repository"


def _collect_candidate_dirs(repo_dir: Path, glob_pattern: str) -> list[str]:
    found: set[str] = set()
    for matched in repo_dir.glob(glob_pattern):
        rel = matched.relative_to(repo_dir)
        if rel == Path("."):
            continue
        found.add(str(rel).replace("\\", "/"))
    return sorted(found)


def _collect_ancestor_dirs(path: str, max_depth: int = 5) -> list[str]:
    out: list[str] = []
    current = Path(path)
    depth = 0
    while current != Path(".") and current.parent != current and depth < max_depth:
        as_posix = str(current).replace("\\", "/")
        if as_posix and as_posix != ".":
            out.append(as_posix)
        current = current.parent
        depth += 1
    return out


def _score_folder(name: str, keywords: list[str], *, prefer_shallow: bool = False) -> int:
    base = name.lower()
    score = 0
    for keyword in keywords:
        if keyword in base:
            score += 3
    if base.startswith(("sym", "foot", "3d", "model")):
        score += 1
    if prefer_shallow:
        depth = base.count("/")
        score -= depth
        if any(tok in base for tok in ("3d", "model", "models", "packages3d")):
            score += 3
    return score


def _pick_best(
    candidates: list[str],
    keywords: list[str],
    fallback: str,
    *,
    prefer_shallow: bool = False,
) -> FolderSuggestion:
    if not candidates:
        return FolderSuggestion(selected=fallback, candidates=[fallback])
    ranked_pairs = sorted(
        [(item, _score_folder(item, keywords, prefer_shallow=prefer_shallow)) for item in candidates],
        key=lambda item: (item[1], -item[0].count("/"), -len(item[0])),
        reverse=True,
    )
    ranked = [item for item, _score in ranked_pairs]
    selected = ranked[0]
    top_score = ranked_pairs[0][1]
    second_score = ranked_pairs[1][1] if len(ranked_pairs) > 1 else ranked_pairs[0][1]
    low_confidence = top_score <= 0 or (top_score - second_score) <= 1
    return FolderSuggestion(
        selected=selected,
        candidates=ranked,
        selected_score=top_score,
        low_confidence=low_confidence,
    )


def suggest_library_mapping(repo_dir: Path) -> MappingSuggestion:
    symbol_candidates = _collect_candidate_dirs(repo_dir, "**/*.kicad_sym")
    symbol_dirs = sorted({str(Path(path).parent).replace("\\", "/") for path in symbol_candidates})

    footprint_candidates = [p for p in _collect_candidate_dirs(repo_dir, "**/*.pretty") if p.endswith(".pretty")]
    model_candidates = _collect_candidate_dirs(repo_dir, "**/*.[sS][tT][eE][pP]")
    model_leaf_dirs = {str(Path(path).parent).replace("\\", "/") for path in model_candidates}
    model_dirs: set[str] = set()
    for leaf in model_leaf_dirs:
        model_dirs.update(_collect_ancestor_dirs(leaf, max_depth=6))
    if not model_dirs:
        model_dirs = model_leaf_dirs

    design_block_candidates = [
        path
        for path in _collect_candidate_dirs(repo_dir, "**/*")
        if "design_block" in path.lower().replace("-", "_")
    ]
    design_block_dirs: set[str] = set()
    for matched in design_block_candidates:
        path = repo_dir / matched
        if path.is_dir():
            design_block_dirs.add(matched)
        else:
            design_block_dirs.add(str(Path(matched).parent).replace("\\", "/"))
    database_candidates = [
        str(path.relative_to(repo_dir)).replace("\\", "/")
        for path in repo_dir.glob("**/g-*.csv")
    ]
    database_dirs = sorted({str(Path(path).parent).replace("\\", "/") for path in database_candidates})

    return MappingSuggestion(
        symbols=_pick_best(symbol_dirs, ["symbol", "sym"], "symbols"),
        footprints=_pick_best(footprint_candidates, ["footprint", "pretty"], "footprints"),
        models3d=_pick_best(
            sorted(model_dirs),
            ["3d", "model", "models", "packages3d"],
            "3d-models",
            prefer_shallow=True,
        ),
        design_blocks=_pick_best(sorted(design_block_dirs), ["design", "block"], "design-blocks"),
        database=_pick_best(database_dirs, ["database", "db"], "database"),
    )
