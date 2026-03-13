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
    QLabel,
    QLineEdit,
    QPushButton,
    QVBoxLayout,
    QWidget,
)

from .error_handler import show_error_dialog
from .ipn import generate_capacitor_ipn, generate_inductor_ipn, generate_resistor_ipn
from .kicad_lib import KiCadLibraryIndex, reference_from_footprint_path, references_from_symbol_file
from .si_parser import format_si_value, parse_power_rating, parse_si_value
from .standard_values import snap_capacitor, snap_inductor, snap_resistor
from .supplier_dialog import SupplierSearchDialog


class SmartAddPartDialog(QDialog):
    def __init__(
        self,
        category: str,
        existing_ipns: set[str],
        supplier_dialog_factory,
        library_index: KiCadLibraryIndex,
        workspace_root: Path,
        reference_handler: Callable[[str, str], str] | None = None,
        parent=None,
    ):
        super().__init__(parent)
        self.category = category
        self.existing_ipns = existing_ipns
        self.supplier_dialog_factory = supplier_dialog_factory
        self.library_index = library_index
        self.workspace_root = workspace_root
        self.reference_handler = reference_handler
        self._result: dict[str, str] = {}
        self.setWindowTitle(f"Smart Add {category}")

        self.mounting_type = QComboBox()
        self.mounting_type.addItems(["SMT", "Through-Hole"])
        self.ideal_value = QLineEdit()
        self.package = QComboBox()
        self.package.setEditable(True)
        self.tolerance = QComboBox()
        self.tolerance.addItems(["1%", "5%", "10%"])
        self.rating = QLineEdit()
        self.rating.setPlaceholderText("Power/Voltage/Current rating")
        self.info = QLabel("")
        self.ipn_preview = QLabel("IPN: (pending)")
        self.mpn = QLineEdit()
        self.manufacturer = QLineEdit()
        self.datasheet = QLineEdit()
        self.digikey_pn = QLineEdit()
        self.mouser_pn = QLineEdit()
        self.symbol = QLineEdit()
        self.footprint = QLineEdit()

        symbol_completer = QCompleter([entry.name for entry in self.library_index.entries("symbol")], self.symbol)
        symbol_completer.setCaseSensitivity(Qt.CaseSensitivity.CaseInsensitive)
        symbol_completer.setFilterMode(Qt.MatchFlag.MatchContains)
        self.symbol.setCompleter(symbol_completer)

        footprint_completer = QCompleter([entry.name for entry in self.library_index.entries("footprint")], self.footprint)
        footprint_completer.setCaseSensitivity(Qt.CaseSensitivity.CaseInsensitive)
        footprint_completer.setFilterMode(Qt.MatchFlag.MatchContains)
        self.footprint.setCompleter(footprint_completer)

        suggest_btn = QPushButton("Compute Closest + IPN")
        suggest_btn.clicked.connect(self.compute)
        supplier_btn = QPushButton("Find Supplier Part")
        supplier_btn.clicked.connect(self.pick_supplier)
        self.mounting_type.currentTextChanged.connect(self._on_mounting_type_changed)

        form = QFormLayout()
        form.addRow("Mounting", self.mounting_type)
        form.addRow("Ideal value", self.ideal_value)
        form.addRow("Package", self.package)
        form.addRow("Tolerance", self.tolerance)
        form.addRow("Rating", self.rating)
        form.addRow("", suggest_btn)
        form.addRow("Nearest", self.info)
        form.addRow("", self.ipn_preview)
        form.addRow("MPN", self.mpn)
        form.addRow("Manufacturer", self.manufacturer)
        form.addRow("Datasheet", self.datasheet)
        form.addRow("DigiKey_PN", self.digikey_pn)
        form.addRow("Mouser_PN", self.mouser_pn)
        form.addRow("Symbol", self._build_browser_row(self.symbol, self._browse_symbol, self._match_symbol))
        form.addRow("Footprint", self._build_browser_row(self.footprint, self._browse_footprint, self._match_footprint))
        form.addRow("", supplier_btn)

        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel)
        buttons.accepted.connect(self.accept)
        buttons.rejected.connect(self.reject)

        layout = QVBoxLayout()
        layout.addLayout(form)
        layout.addWidget(buttons)
        self.setLayout(layout)

        self._on_mounting_type_changed(self.mounting_type.currentText())

    def _build_browser_row(self, edit: QLineEdit, browse_handler, match_handler) -> QWidget:
        row = QWidget(self)
        row_layout = QHBoxLayout(row)
        row_layout.setContentsMargins(0, 0, 0, 0)
        row_layout.addWidget(edit, 1)
        match = QPushButton("↻", row)
        match.setFixedWidth(28)
        match.setToolTip(
            "Find closest match from current inputs "
            "(category, mounting type, package, and typed values)."
        )
        match.clicked.connect(match_handler)
        row_layout.addWidget(match)
        browse = QPushButton("...", row)
        browse.setFixedWidth(28)
        browse.clicked.connect(browse_handler)
        row_layout.addWidget(browse)
        return row

    def _on_mounting_type_changed(self, _value: str) -> None:
        current = self.package.currentText().strip()
        if self.mounting_type.currentText() == "SMT":
            items = ["0402", "0603", "0805", "1206", "1210", "0201", "2010", "2512"]
        else:
            items = ["Axial_DIN0207", "Axial_DIN0411", "Radial_D5.0", "Radial_D6.3", "DO-35", "DO-41"]
        self.package.blockSignals(True)
        self.package.clear()
        self.package.addItems(items)
        if current:
            self.package.setCurrentText(current)
        self.package.blockSignals(False)

    def _tokenize(self, text: str) -> list[str]:
        return [tok for tok in re.split(r"[^A-Za-z0-9]+", text.lower()) if len(tok) >= 2]

    def _context_tokens(self, kind: str) -> list[str]:
        tokens: list[str] = []
        if self.category == "res":
            tokens.extend(["res", "r"])
        elif self.category == "cap":
            tokens.extend(["cap", "c"])
        elif self.category == "ind":
            tokens.extend(["ind", "l"])
        else:
            tokens.append(self.category)
        package = self.package.currentText().strip()
        if package:
            tokens.extend(self._tokenize(package))
        if self.mounting_type.currentText() == "SMT":
            tokens.extend(["smd", "smt", "metric"])
        else:
            tokens.extend(["tht", "through", "hole", "axial", "radial"])
        if kind == "symbol":
            tokens.append("pas" if self.category in {"res", "cap"} else self.category)
        return list(dict.fromkeys(tokens))

    def _pick_match(self, kind: str, title: str) -> str | None:
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
            self.symbol.setText(self._normalize_reference("Symbol", picked))

    def _match_footprint(self) -> None:
        picked = self._pick_match("footprint", "Match Footprint")
        if picked:
            self.footprint.setText(self._normalize_reference("Footprint", picked))

    def _normalize_reference(self, header: str, value: str) -> str:
        if not value:
            return value
        if self.reference_handler:
            return self.reference_handler(header, value)
        kind = "symbol" if header == "Symbol" else "footprint"
        entry = self.library_index.resolve(value, kind)
        return value if not entry else entry.name

    def _browse_symbol(self) -> None:
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
        self.symbol.setText(self._normalize_reference("Symbol", selected_ref))

    def _browse_footprint(self) -> None:
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
        self.footprint.setText(self._normalize_reference("Footprint", ref))

    def pick_supplier(self) -> None:
        dialog: SupplierSearchDialog = self.supplier_dialog_factory(self)
        query = f"{self.package.currentText()} {self.ideal_value.text()} {self.category}"
        dialog.query.setText(query)
        if dialog.exec() != QDialog.DialogCode.Accepted or not dialog.selected_part:
            return
        part = dialog.selected_part
        self.mpn.setText(part.mpn)
        self.manufacturer.setText(part.manufacturer)
        self.datasheet.setText(part.datasheet)
        self.digikey_pn.setText(part.digikey_pn)
        self.mouser_pn.setText(part.mouser_pn)
        if not self.ideal_value.text() and part.description:
            self.ideal_value.setText(part.description)

    def compute(self) -> None:
        try:
            value = parse_si_value(self.ideal_value.text())
            if value <= 0:
                self.info.setText("Enter a valid positive value")
                return
            _rating_watts, rating_text = parse_power_rating(self.rating.text())
            rating_segment = rating_text if rating_text else self.rating.text().strip()

            if self.category == "res":
                snap = snap_resistor(value, self.tolerance.currentText())
                ipn = generate_resistor_ipn(self.existing_ipns, snap.value)
                desc = (
                    f"RES {snap.value:g} OHM {self.tolerance.currentText()} {rating_segment} "
                    f"{self.package.currentText()}"
                ).strip()
                symbol = "g-pas:R_US"
                if self.mounting_type.currentText() == "SMT":
                    footprint = f"g-res:R_{self.package.currentText()}_Metric"
                else:
                    footprint = f"g-res:R_{self.package.currentText()}"
                value_field = format_si_value(snap.value)
            elif self.category == "cap":
                snap = snap_capacitor(value, self.tolerance.currentText())
                ipn = generate_capacitor_ipn(self.existing_ipns, snap.value)
                desc = (
                    f"CAP {snap.value:g}F {self.tolerance.currentText()} {rating_segment} "
                    f"{self.package.currentText()}"
                ).strip()
                symbol = "g-pas:C"
                if self.mounting_type.currentText() == "SMT":
                    footprint = f"g-cap:C_{self.package.currentText()}_Metric"
                else:
                    footprint = f"g-cap:C_{self.package.currentText()}"
                value_field = format_si_value(snap.value)
            else:
                snap = snap_inductor(value)
                ipn = generate_inductor_ipn(self.existing_ipns)
                desc = f"IND {snap.value:g}H {rating_segment} {self.package.currentText()}".strip()
                symbol = "g-ind:L"
                if self.mounting_type.currentText() == "SMT":
                    footprint = f"g-ind:L_{self.package.currentText()}_Metric"
                else:
                    footprint = f"g-ind:L_{self.package.currentText()}"
                value_field = format_si_value(snap.value)

            if not self.symbol.text().strip():
                self.symbol.setText(symbol)
            if not self.footprint.text().strip():
                self.footprint.setText(footprint)

            self.info.setText(f"{value_field} (error {snap.error_percent:.2f}%)")
            self.ipn_preview.setText(f"IPN: {ipn}")
            self._result = {
                "IPN": ipn,
                "Description": desc,
                "Symbol": self._normalize_reference("Symbol", self.symbol.text().strip() or symbol),
                "Footprint": self._normalize_reference("Footprint", self.footprint.text().strip() or footprint),
                "Value": value_field,
                "MPN": self.mpn.text().strip(),
                "Manufacturer": self.manufacturer.text().strip(),
                "Datasheet": self.datasheet.text().strip(),
                "DigiKey_PN": self.digikey_pn.text().strip(),
                "Mouser_PN": self.mouser_pn.text().strip(),
            }
        except Exception as exc:
            self.info.setText(f"Error: {exc}")
            show_error_dialog(self, "Unable to compute part value", str(exc), f"Input: {self.ideal_value.text()}\n{exc}")

    def row_data(self) -> dict[str, str]:
        if not self._result:
            self.compute()
        return self._result

