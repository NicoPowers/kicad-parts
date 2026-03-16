from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from .csv_manager import CsvDocument, read_csv, write_csv
from .ipn import parse_ipn
from .lib_sync import copy_file
from .provider_config import Provider


@dataclass(frozen=True)
class ShareResult:
    copied_rows: int
    copied_assets: int
    skipped: int
    messages: list[str]


def _strip_prefix_ipn(value: str, prefix: str) -> str:
    parsed = parse_ipn(value)
    if parsed and parsed.prefix == prefix:
        return f"{parsed.ccc}-{parsed.nnnn}-{parsed.vvvv}"
    return value


def _load_provider_category_csv(workspace_root: Path, provider: Provider, category: str) -> CsvDocument:
    path = workspace_root / provider.database_path / f"g-{category}.csv"
    if path.exists():
        return read_csv(path)
    return CsvDocument(path=path, headers=[], rows=[], quote_all=False)


def _copy_symbol_asset(workspace_root: Path, src: Provider, dst: Provider, symbol_ref: str) -> int:
    if ":" not in symbol_ref:
        return 0
    library, _name = symbol_ref.split(":", 1)
    src_file = workspace_root / src.symbols_path / f"{library}.kicad_sym"
    dst_file = workspace_root / dst.symbols_path / f"{library}.kicad_sym"
    if not src_file.exists() or dst_file.exists():
        return 0
    result = copy_file(src_file, dst_file)
    return 1 if result.copied else 0


def _copy_footprint_asset(workspace_root: Path, src: Provider, dst: Provider, footprint_ref: str) -> int:
    if ":" not in footprint_ref:
        return 0
    library, name = footprint_ref.split(":", 1)
    src_mod = workspace_root / src.footprints_path / f"{library}.pretty" / f"{name}.kicad_mod"
    dst_mod = workspace_root / dst.footprints_path / f"{library}.pretty" / f"{name}.kicad_mod"
    if not src_mod.exists() or dst_mod.exists():
        return 0
    result = copy_file(src_mod, dst_mod)
    return 1 if result.copied else 0


def share_parts_between_providers(
    workspace_root: Path,
    category: str,
    source_provider: Provider,
    destination_provider: Provider,
    ipns: set[str],
) -> ShareResult:
    src_doc = _load_provider_category_csv(workspace_root, source_provider, category)
    dst_doc = _load_provider_category_csv(workspace_root, destination_provider, category)
    if not dst_doc.headers:
        dst_doc.headers = src_doc.headers.copy()

    existing = {row.get("IPN", "").strip() for row in dst_doc.rows}
    copied_rows = 0
    copied_assets = 0
    skipped = 0
    messages: list[str] = []

    for row in src_doc.rows:
        raw_ipn = row.get("IPN", "").strip()
        prefixed_ipn = f"{source_provider.prefix}-{raw_ipn}" if raw_ipn else ""
        if ipns and raw_ipn not in ipns and prefixed_ipn not in ipns:
            continue
        normalized_ipn = _strip_prefix_ipn(raw_ipn, source_provider.prefix)
        if normalized_ipn in existing:
            skipped += 1
            continue
        cloned = dict(row)
        cloned["IPN"] = normalized_ipn
        dst_doc.rows.append(cloned)
        existing.add(normalized_ipn)
        copied_rows += 1
        copied_assets += _copy_symbol_asset(workspace_root, source_provider, destination_provider, cloned.get("Symbol", ""))
        copied_assets += _copy_footprint_asset(
            workspace_root, source_provider, destination_provider, cloned.get("Footprint", "")
        )

    write_csv(dst_doc, make_backup=True)
    messages.append(
        f"Shared {copied_rows} row(s), copied {copied_assets} asset(s), skipped {skipped} duplicate row(s)."
    )
    return ShareResult(copied_rows=copied_rows, copied_assets=copied_assets, skipped=skipped, messages=messages)
