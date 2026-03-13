from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path


SYMBOL_DEF_RE = re.compile(r'\(symbol\s+"([^"]+)"')


@dataclass(frozen=True)
class LibraryEntry:
    name: str
    source: str
    kind: str
    lib_path: Path
    file_path: Path


def _normalized_symbol_name(raw_name: str) -> str:
    # Child units are encoded like "Sym_1_1" and should not appear as separate picks.
    parts = raw_name.rsplit("_", 2)
    if len(parts) == 3 and parts[-1].isdigit() and parts[-2].isdigit():
        return parts[0]
    return raw_name


def _iter_symbol_names(sym_file: Path) -> set[str]:
    try:
        text = sym_file.read_text(encoding="utf-8", errors="ignore")
    except OSError:
        return set()
    names: set[str] = set()
    for match in SYMBOL_DEF_RE.finditer(text):
        names.add(_normalized_symbol_name(match.group(1).split(":")[-1]))
    return names


def index_symbol_libraries(symbols_dir: Path, source: str) -> list[LibraryEntry]:
    entries: list[LibraryEntry] = []
    if not symbols_dir.exists():
        return entries
    for sym_file in sorted(symbols_dir.glob("*.kicad_sym")):
        lib_name = sym_file.stem
        for symbol_name in sorted(_iter_symbol_names(sym_file)):
            entries.append(
                LibraryEntry(
                    name=f"{lib_name}:{symbol_name}",
                    source=source,
                    kind="symbol",
                    lib_path=sym_file,
                    file_path=sym_file,
                )
            )
    return entries


def index_footprint_libraries(footprints_dir: Path, source: str) -> list[LibraryEntry]:
    entries: list[LibraryEntry] = []
    if not footprints_dir.exists():
        return entries
    for pretty_dir in sorted(footprints_dir.glob("*.pretty")):
        lib_name = pretty_dir.stem
        for footprint_file in sorted(pretty_dir.glob("*.kicad_mod")):
            entries.append(
                LibraryEntry(
                    name=f"{lib_name}:{footprint_file.stem}",
                    source=source,
                    kind="footprint",
                    lib_path=pretty_dir,
                    file_path=footprint_file,
                )
            )
    return entries


def reference_from_footprint_path(footprint_file: Path) -> str | None:
    if footprint_file.suffix.lower() != ".kicad_mod":
        return None
    if not footprint_file.parent.name.endswith(".pretty"):
        return None
    return f"{footprint_file.parent.stem}:{footprint_file.stem}"


def references_from_symbol_file(sym_file: Path) -> list[str]:
    if sym_file.suffix.lower() != ".kicad_sym":
        return []
    symbol_names = sorted(_iter_symbol_names(sym_file))
    return [f"{sym_file.stem}:{name}" for name in symbol_names]


class KiCadLibraryIndex:
    def __init__(self, workspace_root: Path):
        self.workspace_root = workspace_root
        self._symbols: list[LibraryEntry] = []
        self._footprints: list[LibraryEntry] = []

    def rebuild(self) -> None:
        local_symbols = index_symbol_libraries(self.workspace_root / "symbols", source="local")
        local_footprints = index_footprint_libraries(self.workspace_root / "footprints", source="local")
        kicad_symbols = index_symbol_libraries(self.workspace_root / "libs" / "kicad-symbols", source="kicad")
        kicad_footprints = index_footprint_libraries(
            self.workspace_root / "libs" / "kicad-footprints",
            source="kicad",
        )

        self._symbols = self._dedupe(local_symbols + kicad_symbols)
        self._footprints = self._dedupe(local_footprints + kicad_footprints)

    def _dedupe(self, entries: list[LibraryEntry]) -> list[LibraryEntry]:
        out: list[LibraryEntry] = []
        seen: set[tuple[str, str]] = set()
        # Local-first behavior: preserve first occurrence, local is indexed first.
        for entry in entries:
            key = (entry.kind, entry.name.lower())
            if key in seen:
                continue
            seen.add(key)
            out.append(entry)
        return out

    def entries(self, kind: str) -> list[LibraryEntry]:
        return self._symbols if kind == "symbol" else self._footprints

    def search(self, query: str, kind: str, limit: int = 300) -> list[LibraryEntry]:
        needle = query.strip().lower()
        candidates = self.entries(kind)
        if not needle:
            return candidates[:limit]
        starts = [entry for entry in candidates if entry.name.lower().startswith(needle)]
        contains = [entry for entry in candidates if needle in entry.name.lower() and not entry.name.lower().startswith(needle)]
        return (starts + contains)[:limit]

    def fuzzy_match(self, query_tokens: list[str], kind: str, limit: int = 300) -> list[LibraryEntry]:
        tokens = [tok.strip().lower() for tok in query_tokens if tok and tok.strip()]
        if not tokens:
            return self.entries(kind)[:limit]
        scored: list[tuple[int, int, LibraryEntry]] = []
        for entry in self.entries(kind):
            name = entry.name.lower()
            score = sum(1 for tok in tokens if tok in name)
            if score <= 0:
                continue
            # Higher score first; prefer shorter names for similar score.
            scored.append((score, -len(name), entry))
        scored.sort(key=lambda item: (item[0], item[1]), reverse=True)
        return [entry for _score, _len_score, entry in scored[:limit]]

    def resolve(self, name: str, kind: str) -> LibraryEntry | None:
        wanted = name.strip().lower()
        for entry in self.entries(kind):
            if entry.name.lower() == wanted:
                return entry
        return None

    def local_symbol_libraries(self) -> list[str]:
        return sorted(p.stem for p in (self.workspace_root / "symbols").glob("*.kicad_sym"))

    def local_footprint_libraries(self) -> list[str]:
        return sorted(p.stem for p in (self.workspace_root / "footprints").glob("*.pretty"))
