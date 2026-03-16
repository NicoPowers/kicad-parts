from __future__ import annotations

import csv
import os
import shutil
import subprocess
import tempfile
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from .db_generator import generate_sqlite
from .provider_config import Provider, ProviderRegistry
from .schema import REQUIRED_COLUMNS


@dataclass(frozen=True)
class AggregateResult:
    ok: bool
    warnings: list[str]
    errors: list[str]
    stats: dict[str, int]


def _resolve_provider_path(workspace_root: Path, provider: Provider, rel: Path) -> Path:
    path = workspace_root / rel
    return path.resolve()


def _run_cmd(command: list[str], cwd: Path | None = None) -> tuple[int, str]:
    proc = subprocess.run(command, cwd=cwd, capture_output=True, text=True, check=False)
    output = ((proc.stdout or "") + "\n" + (proc.stderr or "")).strip()
    return proc.returncode, output


def _is_windows() -> bool:
    return os.name == "nt"


def _create_dir_link(target: Path, link_path: Path) -> None:
    if not target.exists():
        raise FileNotFoundError(f"Target folder does not exist: {target}")
    if link_path.exists() or link_path.is_symlink():
        _remove_dir_link(link_path)
    link_path.parent.mkdir(parents=True, exist_ok=True)
    try:
        os.symlink(str(target), str(link_path), target_is_directory=True)
        return
    except OSError:
        if not _is_windows():
            raise
    code, output = _run_cmd(["cmd", "/c", "mklink", "/J", str(link_path), str(target)])
    if code != 0:
        raise RuntimeError(output or f"Unable to create junction: {link_path} -> {target}")


def _remove_dir_link(link_path: Path) -> None:
    if not link_path.exists() and not link_path.is_symlink():
        return
    if link_path.is_symlink():
        link_path.unlink(missing_ok=True)
        return
    if _is_windows():
        # Junctions appear as directories but should be removed with rmdir.
        try:
            os.rmdir(link_path)
            return
        except OSError:
            pass
    if link_path.is_dir():
        shutil.rmtree(link_path)
    else:
        link_path.unlink(missing_ok=True)


def _provider_base_dirs(workspace_root: Path, provider: Provider) -> dict[str, Path]:
    return {
        "symbols": _resolve_provider_path(workspace_root, provider, provider.symbols_path),
        "footprints": _resolve_provider_path(workspace_root, provider, provider.footprints_path),
        "models3d": _resolve_provider_path(workspace_root, provider, provider.models3d_path),
        "design-blocks": _resolve_provider_path(workspace_root, provider, provider.design_blocks_path),
        "database": _resolve_provider_path(workspace_root, provider, provider.database_path),
    }


def _provider_repo_exists(workspace_root: Path, provider: Provider) -> bool:
    repo_dir = _resolve_provider_path(workspace_root, provider, provider.repo_path)
    return repo_dir.exists()


def rebuild_aggregate_links(workspace_root: Path, registry: ProviderRegistry) -> list[str]:
    warnings: list[str] = []
    aggregate_roots = {
        "symbols": workspace_root / "symbols",
        "footprints": workspace_root / "footprints",
        "3d-models": workspace_root / "3d-models",
        "design-blocks": workspace_root / "design-blocks",
    }
    for root in aggregate_roots.values():
        root.mkdir(parents=True, exist_ok=True)

    desired_links: set[Path] = set()
    for provider in registry.writable_providers():
        if not _provider_repo_exists(workspace_root, provider):
            warnings.append(
                f"Skipping provider '{provider.id}' while linking aggregate directories: checkout missing at "
                f"{_resolve_provider_path(workspace_root, provider, provider.repo_path)}"
            )
            continue
        dirs = _provider_base_dirs(workspace_root, provider)
        prefix = provider.prefix
        mapping = {
            aggregate_roots["symbols"] / prefix: dirs["symbols"],
            aggregate_roots["footprints"] / prefix: dirs["footprints"],
            aggregate_roots["3d-models"] / prefix: dirs["models3d"],
            aggregate_roots["design-blocks"] / prefix: dirs["design-blocks"],
        }
        for link_path, target in mapping.items():
            desired_links.add(link_path)
            try:
                _create_dir_link(target, link_path)
            except Exception as exc:
                warnings.append(f"Unable to link {link_path} -> {target}: {exc}")

    for root in aggregate_roots.values():
        for child in root.iterdir():
            if child not in desired_links:
                try:
                    _remove_dir_link(child)
                except Exception as exc:
                    warnings.append(f"Unable to remove stale aggregate link {child}: {exc}")
    return warnings


def validate_provider_csvs(provider: Provider, workspace_root: Path) -> list[str]:
    errors: list[str] = []
    if not provider.has_parts():
        return errors
    db_dir = _resolve_provider_path(workspace_root, provider, provider.database_path)
    if not db_dir.exists():
        return [f"Provider '{provider.id}' database path does not exist: {db_dir}"]
    csv_paths = sorted(db_dir.glob("g-*.csv"))
    if not csv_paths:
        return [f"Provider '{provider.id}' has no g-*.csv files in {db_dir}"]
    for csv_path in csv_paths:
        with csv_path.open("r", encoding="utf-8", newline="") as handle:
            reader = csv.reader(handle)
            headers = next(reader, [])
        missing = sorted(REQUIRED_COLUMNS.difference(headers))
        if missing:
            errors.append(f"{provider.id}:{csv_path.name} missing required columns: {', '.join(missing)}")
    return errors


def _prefix_ref(value: str, prefix: str) -> str:
    if ":" not in value:
        return value
    lib, name = value.split(":", 1)
    wanted = f"{prefix}-{lib}"
    if lib.startswith(f"{prefix}-"):
        return value
    return f"{wanted}:{name}"


def _prefix_ipn(value: str, prefix: str) -> str:
    raw = value.strip()
    if not raw:
        return raw
    if raw.count("-") == 3 and raw.startswith(f"{prefix}-"):
        return raw
    return f"{prefix}-{raw}"


def merge_provider_csvs(
    workspace_root: Path,
    registry: ProviderRegistry,
    progress_cb: Callable[[str], None] | None = None,
) -> tuple[list[str], dict[str, int]]:
    warnings: list[str] = []
    by_category: dict[str, tuple[list[str], list[dict[str, str]]]] = {}
    total_rows = 0
    provider_count = 0

    for provider in registry.writable_providers():
        if not _provider_repo_exists(workspace_root, provider):
            warnings.append(
                f"Skipping provider '{provider.id}' while merging CSVs: checkout missing at "
                f"{_resolve_provider_path(workspace_root, provider, provider.repo_path)}"
            )
            continue
        db_dir = _resolve_provider_path(workspace_root, provider, provider.database_path)
        if not db_dir.exists():
            warnings.append(
                f"Skipping provider '{provider.id}' while merging CSVs: database path does not exist: {db_dir}"
            )
            continue
        provider_count += 1
        csv_paths = sorted(db_dir.glob("g-*.csv"))
        for csv_path in csv_paths:
            if progress_cb:
                progress_cb(f"Merging {provider.id}/{csv_path.name}...")
            with csv_path.open("r", encoding="utf-8", newline="") as handle:
                reader = csv.DictReader(handle)
                headers = list(reader.fieldnames or [])
                rows = list(reader)
            key = csv_path.name
            if key not in by_category:
                by_category[key] = (headers, [])
            expected_headers, out_rows = by_category[key]
            if expected_headers != headers:
                warnings.append(
                    f"Header mismatch in {provider.id}:{csv_path.name}; expected {expected_headers}, got {headers}"
                )
                continue
            for row in rows:
                mapped = dict(row)
                mapped["IPN"] = _prefix_ipn(mapped.get("IPN", ""), provider.prefix)
                mapped["Symbol"] = _prefix_ref(mapped.get("Symbol", ""), provider.prefix)
                mapped["Footprint"] = _prefix_ref(mapped.get("Footprint", ""), provider.prefix)
                out_rows.append(mapped)
                total_rows += 1

    database_dir = workspace_root / "database"
    database_dir.mkdir(parents=True, exist_ok=True)
    for name, (headers, rows) in by_category.items():
        out_path = database_dir / name
        with out_path.open("w", encoding="utf-8", newline="") as handle:
            writer = csv.DictWriter(handle, fieldnames=headers)
            writer.writeheader()
            writer.writerows(rows)

    return warnings, {
        "providers_merged": provider_count,
        "categories_merged": len(by_category),
        "rows_merged": total_rows,
    }


def check_write_access(provider: Provider, workspace_root: Path) -> bool:
    repo_dir = _resolve_provider_path(workspace_root, provider, provider.repo_path)
    repo_dir.mkdir(parents=True, exist_ok=True)
    probe_path = Path(tempfile.mkstemp(prefix=".write-probe-", suffix=".tmp", dir=str(repo_dir))[1])
    try:
        probe_path.write_text("ok", encoding="utf-8")
    finally:
        probe_path.unlink(missing_ok=True)
    if provider.repo_url:
        code, _output = _run_cmd(["git", "push", "--dry-run"], cwd=repo_dir)
        return code == 0
    return True


def build_kicad_table_entries(workspace_root: Path, registry: ProviderRegistry) -> tuple[list[str], list[str]]:
    symbol_entries: list[str] = []
    footprint_entries: list[str] = []
    for provider in registry.writable_providers():
        symbol_root = workspace_root / "symbols" / provider.prefix
        for sym_file in sorted(symbol_root.glob("*.kicad_sym")):
            nickname = f"{provider.prefix}-{sym_file.stem}"
            symbol_entries.append(f'{nickname}="{sym_file.as_posix()}"')
        footprint_root = workspace_root / "footprints" / provider.prefix
        for pretty_dir in sorted(footprint_root.glob("*.pretty")):
            nickname = f"{provider.prefix}-{pretty_dir.stem}"
            footprint_entries.append(f'{nickname}="{pretty_dir.as_posix()}"')
    return symbol_entries, footprint_entries


def write_kicad_table_fragments(workspace_root: Path, registry: ProviderRegistry) -> Path:
    symbol_entries, footprint_entries = build_kicad_table_entries(workspace_root, registry)
    target = workspace_root / "database" / "provider-lib-table-fragments.txt"
    target.parent.mkdir(parents=True, exist_ok=True)
    lines = ["# sym-lib-table entries"]
    lines.extend(symbol_entries)
    lines.append("")
    lines.append("# fp-lib-table entries")
    lines.extend(footprint_entries)
    target.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return target


def rebuild_aggregate(
    workspace_root: Path,
    registry: ProviderRegistry,
    progress_cb: Callable[[str], None] | None = None,
) -> AggregateResult:
    warnings: list[str] = []
    errors: list[str] = []

    if progress_cb:
        progress_cb("Validating provider CSV schemas...")
    for provider in registry.writable_providers():
        if not _provider_repo_exists(workspace_root, provider):
            warnings.append(
                f"Skipping provider '{provider.id}' during validation: checkout missing at "
                f"{_resolve_provider_path(workspace_root, provider, provider.repo_path)}"
            )
            continue
        db_dir = _resolve_provider_path(workspace_root, provider, provider.database_path)
        if not db_dir.exists():
            warnings.append(
                f"Skipping provider '{provider.id}' during validation: database path does not exist: {db_dir}"
            )
            continue
        errors.extend(validate_provider_csvs(provider, workspace_root))
    if errors:
        return AggregateResult(ok=False, warnings=warnings, errors=errors, stats={})

    if progress_cb:
        progress_cb("Creating aggregate links...")
    warnings.extend(rebuild_aggregate_links(workspace_root, registry))

    if progress_cb:
        progress_cb("Merging provider CSVs...")
    merge_warnings, stats = merge_provider_csvs(workspace_root, registry, progress_cb=progress_cb)
    warnings.extend(merge_warnings)

    if progress_cb:
        progress_cb("Generating SQLite database...")
    out_db = workspace_root / "database" / "parts.sqlite"
    generate_sqlite(workspace_root / "database", out_db)

    write_kicad_table_fragments(workspace_root, registry)
    return AggregateResult(ok=True, warnings=warnings, errors=[], stats=stats)
