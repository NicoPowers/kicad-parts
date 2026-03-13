from __future__ import annotations

import re
import shutil
import urllib.request
from dataclasses import dataclass
from pathlib import Path


MODEL_REF_RE = re.compile(r"\$\{KICAD[0-9_]*3DMODEL_DIR\}/([^/\"]+\.3dshapes)/([^\"]+\.(?:step|stp|wrl))", re.IGNORECASE)
SYMBOL_HEADER_RE = re.compile(r"\(symbol\s+\"([^\"]+)\"")


@dataclass(frozen=True)
class CopyResult:
    copied: bool
    path: Path
    message: str = ""


def _extract_symbol_block(text: str, symbol_name: str) -> str | None:
    pattern = re.compile(rf'\(symbol\s+"(?:[^":]+:)?{re.escape(symbol_name)}"(?:\s|\))')
    match = pattern.search(text)
    if not match:
        return None
    start = match.start()
    depth = 0
    in_string = False
    escape = False
    for idx in range(start, len(text)):
        ch = text[idx]
        if in_string:
            if escape:
                escape = False
            elif ch == "\\":
                escape = True
            elif ch == '"':
                in_string = False
            continue
        if ch == '"':
            in_string = True
            continue
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
            if depth == 0:
                return text[start : idx + 1]
    return None


def _retarget_symbol_namespace(symbol_block: str, symbol_name: str, target_library: str) -> str:
    pattern = re.compile(rf'(\(symbol\s+")([^"]*:)?{re.escape(symbol_name)}(")')
    return pattern.sub(rf'\1{target_library}:{symbol_name}\3', symbol_block, count=1)


def copy_symbol(src_sym_path: Path, symbol_name: str, dest_sym_path: Path) -> CopyResult:
    source_text = src_sym_path.read_text(encoding="utf-8")
    symbol_block = _extract_symbol_block(source_text, symbol_name)
    if not symbol_block:
        return CopyResult(False, dest_sym_path, f"Symbol '{symbol_name}' not found in {src_sym_path.name}")
    symbol_block = _retarget_symbol_namespace(symbol_block, symbol_name, dest_sym_path.stem)

    if dest_sym_path.exists():
        existing = dest_sym_path.read_text(encoding="utf-8")
        for existing_name in SYMBOL_HEADER_RE.findall(existing):
            if existing_name.split(":")[-1] == symbol_name:
                return CopyResult(False, dest_sym_path, "Symbol already exists in local library")
        insert_at = existing.rfind(")")
        if insert_at < 0:
            return CopyResult(False, dest_sym_path, "Invalid destination .kicad_sym format")
        new_text = existing[:insert_at].rstrip() + "\n  " + symbol_block + "\n)\n"
    else:
        dest_sym_path.parent.mkdir(parents=True, exist_ok=True)
        new_text = (
            '(kicad_symbol_lib\n'
            '  (version 20230121)\n'
            '  (generator "kicad_parts_gui")\n'
            f'  {symbol_block}\n'
            ')\n'
        )

    dest_sym_path.write_text(new_text, encoding="utf-8")
    return CopyResult(True, dest_sym_path)


def fetch_3d_model(model_ref: str, local_3d_dir: Path) -> CopyResult:
    m = MODEL_REF_RE.search(model_ref)
    if not m:
        return CopyResult(False, local_3d_dir, "Unsupported 3D model reference")
    lib_3d, model_name = m.groups()
    local_3d_dir.mkdir(parents=True, exist_ok=True)
    target = local_3d_dir / Path(model_name).name
    if target.exists():
        return CopyResult(False, target, "3D model already exists")

    url = f"https://gitlab.com/kicad/libraries/kicad-packages3D/-/raw/master/{lib_3d}/{model_name}"
    with urllib.request.urlopen(url) as response:  # nosec B310
        payload = response.read()
    target.write_bytes(payload)
    return CopyResult(True, target)


def _rewrite_model_refs(text: str, local_3d_dir: Path) -> tuple[str, list[CopyResult]]:
    results: list[CopyResult] = []

    def replace(match: re.Match[str]) -> str:
        full_ref = match.group(0)
        _, model_name = match.groups()
        fetch_result = fetch_3d_model(full_ref, local_3d_dir)
        results.append(fetch_result)
        return "${GITPLM_PARTS}/3d-models/" + Path(model_name).name

    rewritten = MODEL_REF_RE.sub(replace, text)
    return rewritten, results


def copy_footprint(src_mod_path: Path, dest_pretty_dir: Path, local_3d_dir: Path) -> tuple[CopyResult, list[CopyResult]]:
    dest_pretty_dir.mkdir(parents=True, exist_ok=True)
    dest_mod = dest_pretty_dir / src_mod_path.name
    if dest_mod.exists():
        return CopyResult(False, dest_mod, "Footprint already exists in local library"), []

    text = src_mod_path.read_text(encoding="utf-8", errors="ignore")
    rewritten_text, model_results = _rewrite_model_refs(text, local_3d_dir)
    dest_mod.write_text(rewritten_text, encoding="utf-8")
    return CopyResult(True, dest_mod), model_results


def copy_file(src: Path, dst: Path) -> CopyResult:
    dst.parent.mkdir(parents=True, exist_ok=True)
    if dst.exists():
        return CopyResult(False, dst, "File already exists")
    shutil.copy2(src, dst)
    return CopyResult(True, dst)
