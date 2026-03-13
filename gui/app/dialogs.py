from __future__ import annotations

import re
from pathlib import Path
from typing import Callable

from PyQt6.QtCore import Qt
from PyQt6.QtWidgets import (
    QCompleter,
    QComboBox,
    QDialog,
    QDialogButtonBox,
    QFileDialog,
    QFormLayout,
    QHBoxLayout,
    QInputDialog,
    QLineEdit,
    QPushButton,
    QVBoxLayout,
    QWidget,
)

from .kicad_lib import KiCadLibraryIndex, reference_from_footprint_path, references_from_symbol_file
from .supplier_dialog import SupplierSearchDialog


class GenericAddPartDialog(QDialog):
    def __init__(
        self,
        headers: list[str],
        defaults: dict[str, str] | None = None,
        completer_values: dict[str, list[str]] | None = None,
        workspace_root: Path | None = None,
        library_index: KiCadLibraryIndex | None = None,
        reference_handler: Callable[[str, str], str] | None = None,
        category: str | None = None,
        supplier_dialog_factory=None,
        parent=None,
    ):
        super().__init__(parent)
        self.setWindowTitle("Add Part")
        self._fields: dict[str, QLineEdit] = {}
        self.workspace_root = workspace_root
        self.library_index = library_index
        self.reference_handler = reference_handler
        self.category = (category or "").strip().lower()
        self.supplier_dialog_factory = supplier_dialog_factory
        self.mounting_type = QComboBox(self)
        self.mounting_type.addItems(["SMT", "Through-Hole"])
        defaults = defaults or {}
        completer_values = completer_values or {}

        form = QFormLayout()
        self.has_symbol_or_footprint = "Symbol" in headers or "Footprint" in headers
        if self.has_symbol_or_footprint:
            form.addRow("Mounting", self.mounting_type)
        for header in headers:
            edit = QLineEdit(defaults.get(header, ""))
            values = completer_values.get(header, [])
            if values:
                completer = QCompleter(values, edit)
                completer.setCaseSensitivity(Qt.CaseSensitivity.CaseInsensitive)
                completer.setFilterMode(Qt.MatchFlag.MatchContains)
                edit.setCompleter(completer)
            self._fields[header] = edit
            if header == "Symbol":
                form.addRow(header, self._build_browser_row(edit, self._browse_symbol, self._match_symbol))
            elif header == "Footprint":
                form.addRow(header, self._build_browser_row(edit, self._browse_footprint, self._match_footprint))
            else:
                form.addRow(header, edit)

        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel)
        buttons.accepted.connect(self.accept)
        buttons.rejected.connect(self.reject)

        layout = QVBoxLayout()
        layout.addLayout(form)
        if self.supplier_dialog_factory:
            supplier_btn = QPushButton("Find Supplier Part")
            supplier_btn.clicked.connect(self.pick_supplier)
            layout.addWidget(supplier_btn)
        layout.addWidget(buttons)
        self.setLayout(layout)

    def _build_browser_row(self, edit: QLineEdit, browse_handler, match_handler) -> QWidget:
        row = QWidget(self)
        row_layout = QHBoxLayout(row)
        row_layout.setContentsMargins(0, 0, 0, 0)
        row_layout.addWidget(edit, 1)
        match = QPushButton("↻", row)
        match.setFixedWidth(28)
        match.setToolTip(
            "Find closest match from current inputs "
            "(category, mounting type, description/type/value/package)."
        )
        match.clicked.connect(match_handler)
        row_layout.addWidget(match)
        browse = QPushButton("...", row)
        browse.setFixedWidth(28)
        browse.clicked.connect(browse_handler)
        row_layout.addWidget(browse)
        return row

    def _tokenize(self, text: str) -> list[str]:
        return [tok for tok in re.split(r"[^A-Za-z0-9]+", text.lower()) if len(tok) >= 2]

    def _context_tokens(self, kind: str) -> list[str]:
        tokens: list[str] = []
        if self.category:
            tokens.extend(self._tokenize(self.category))
        if self.mounting_type.currentText() == "SMT":
            tokens.extend(["smd", "smt", "metric"])
        else:
            tokens.extend(["tht", "through", "hole", "axial", "radial"])
        for field_name in ("Description", "Type", "Value", "Package"):
            if field_name in self._fields:
                tokens.extend(self._tokenize(self._fields[field_name].text()))
        if kind == "symbol":
            if self.category == "res":
                tokens.extend(["r", "res", "pas"])
            elif self.category == "cap":
                tokens.extend(["c", "cap", "pas"])
            elif self.category == "ind":
                tokens.extend(["l", "ind"])
        return list(dict.fromkeys(tokens))

    def _pick_match(self, kind: str, title: str) -> str | None:
        if not self.library_index:
            return None
        tokens = self._context_tokens(kind)
        matches = self.library_index.fuzzy_match(tokens, kind, limit=40)
        if not matches and tokens:
            matches = self.library_index.search(tokens[0], kind, limit=40)
        if not matches:
            return None
        names = [entry.name for entry in matches]
        if len(names) == 1:
            return names[0]
        selected, ok = QInputDialog.getItem(self, title, "Select match", names, 0, False)
        if not ok:
            return None
        return selected

    def _match_symbol(self) -> None:
        picked = self._pick_match("symbol", "Match Symbol")
        if picked:
            self._fields["Symbol"].setText(self._normalize_reference("Symbol", picked))

    def _match_footprint(self) -> None:
        picked = self._pick_match("footprint", "Match Footprint")
        if picked:
            self._fields["Footprint"].setText(self._normalize_reference("Footprint", picked))

    def _normalize_reference(self, header: str, value: str) -> str:
        if not value:
            return value
        if self.reference_handler:
            return self.reference_handler(header, value)
        if not self.library_index:
            return value
        kind = "symbol" if header == "Symbol" else "footprint"
        entry = self.library_index.resolve(value, kind)
        return value if not entry else entry.name

    def _browse_symbol(self) -> None:
        if not self.workspace_root:
            return
        start_dir = self.workspace_root / "symbols"
        if not start_dir.exists():
            start_dir = self.workspace_root / "libs" / "kicad-symbols"
        selected, _ = QFileDialog.getOpenFileName(
            self,
            "Select Symbol Library",
            str(start_dir),
            "KiCad Symbol (*.kicad_sym)",
        )
        if not selected:
            return
        refs = references_from_symbol_file(Path(selected))
        if not refs:
            return
        selected_ref = refs[0]
        if len(refs) > 1:
            selected_ref, ok = QInputDialog.getItem(self, "Select Symbol", "Symbol", refs, 0, False)
            if not ok:
                return
        self._fields["Symbol"].setText(self._normalize_reference("Symbol", selected_ref))

    def _browse_footprint(self) -> None:
        if not self.workspace_root:
            return
        start_dir = self.workspace_root / "footprints"
        if not start_dir.exists():
            start_dir = self.workspace_root / "libs" / "kicad-footprints"
        selected, _ = QFileDialog.getOpenFileName(
            self,
            "Select Footprint",
            str(start_dir),
            "KiCad Footprint (*.kicad_mod)",
        )
        if not selected:
            return
        ref = reference_from_footprint_path(Path(selected))
        if not ref:
            return
        self._fields["Footprint"].setText(self._normalize_reference("Footprint", ref))

    def row_data(self) -> dict[str, str]:
        return {key: field.text().strip() for key, field in self._fields.items()}

    def pick_supplier(self) -> None:
        if not self.supplier_dialog_factory:
            return
        dialog: SupplierSearchDialog = self.supplier_dialog_factory(self)
        query_parts = [
            self._fields["Description"].text() if "Description" in self._fields else "",
            self._fields["Type"].text() if "Type" in self._fields else "",
            self._fields["Value"].text() if "Value" in self._fields else "",
            self._fields["Package"].text() if "Package" in self._fields else "",
            self.category,
        ]
        dialog.query.setText(" ".join(part.strip() for part in query_parts if part and part.strip()))
        if dialog.exec() != QDialog.DialogCode.Accepted or not dialog.selected_part:
            return
        part = dialog.selected_part
        if "MPN" in self._fields:
            self._fields["MPN"].setText(part.mpn)
        if "Manufacturer" in self._fields:
            self._fields["Manufacturer"].setText(part.manufacturer)
        if "Datasheet" in self._fields:
            self._fields["Datasheet"].setText(part.datasheet)
        if "DigiKey_PN" in self._fields:
            self._fields["DigiKey_PN"].setText(part.digikey_pn)
        if "Mouser_PN" in self._fields:
            self._fields["Mouser_PN"].setText(part.mouser_pn)

