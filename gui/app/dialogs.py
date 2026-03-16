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
    QLabel,
    QInputDialog,
    QLineEdit,
    QListWidget,
    QMessageBox,
    QPushButton,
    QVBoxLayout,
    QWidget,
)

from .kicad_lib import KiCadLibraryIndex, reference_from_footprint_path, references_from_symbol_file
from .provider_sync import MappingSuggestion, sanitize_provider_id
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
            if self.library_index:
                for entry in self.library_index.entries("symbol"):
                    if entry.source != "local":
                        start_dir = entry.file_path.parent
                        break
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
            if self.library_index:
                for entry in self.library_index.entries("footprint"):
                    if entry.source != "local":
                        start_dir = entry.file_path.parent
                        break
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


class ProviderMappingDialog(QDialog):
    def __init__(
        self,
        *,
        repo_url: str,
        provider_name: str,
        repo_path: str,
        suggestion: MappingSuggestion,
        parent=None,
    ):
        super().__init__(parent)
        self.setWindowTitle("Provider Folder Mapping")
        self._repo_url = QLineEdit(repo_url)
        self._repo_url.setReadOnly(True)
        self._provider_name = QLineEdit(provider_name)
        self._prefix = QLineEdit("")
        self._prefix.setPlaceholderText("2-3 uppercase letters (e.g. SL)")
        suggested_prefix = "".join(part[:1] for part in re.split(r"[^A-Za-z]+", provider_name) if part)[:3].upper()
        self._prefix.setText((suggested_prefix or provider_name[:3]).upper())
        self._repo_path = QLineEdit(repo_path)
        self._repo_path.setReadOnly(True)

        self._symbols = QComboBox(self)
        self._symbols.setEditable(True)
        self._symbols.addItems(suggestion.symbols.candidates)
        self._symbols.setCurrentText(suggestion.symbols.selected)

        self._footprints = QComboBox(self)
        self._footprints.setEditable(True)
        self._footprints.addItems(suggestion.footprints.candidates)
        self._footprints.setCurrentText(suggestion.footprints.selected)

        self._models = QComboBox(self)
        self._models.setEditable(True)
        self._models.addItems(suggestion.models3d.candidates)
        self._models.setCurrentText(suggestion.models3d.selected)

        self._design_blocks = QComboBox(self)
        self._design_blocks.setEditable(True)
        self._design_blocks.addItems(suggestion.design_blocks.candidates)
        self._design_blocks.setCurrentText(suggestion.design_blocks.selected)

        self._database = QComboBox(self)
        self._database.setEditable(True)
        self._database.addItems(suggestion.database.candidates)
        self._database.setCurrentText(suggestion.database.selected)

        symbols_hint = QLabel(self)
        symbols_hint.setWordWrap(True)
        symbols_hint.setText(
            "Symbol folder auto-detection has low confidence; verify selection."
            if suggestion.symbols.low_confidence
            else "Symbol folder auto-detected."
        )
        footprints_hint = QLabel(self)
        footprints_hint.setWordWrap(True)
        footprints_hint.setText(
            "Footprint folder auto-detection has low confidence; verify selection."
            if suggestion.footprints.low_confidence
            else "Footprint folder auto-detected."
        )
        models_hint = QLabel(self)
        models_hint.setWordWrap(True)
        models_hint.setText(
            "3D folder auto-detection has low confidence; verify selection."
            if suggestion.models3d.low_confidence
            else "3D folder auto-detected."
        )
        design_blocks_hint = QLabel(self)
        design_blocks_hint.setWordWrap(True)
        design_blocks_hint.setText(
            "Design blocks folder auto-detection has low confidence; verify selection."
            if suggestion.design_blocks.low_confidence
            else "Design blocks folder auto-detected."
        )
        database_hint = QLabel(self)
        database_hint.setWordWrap(True)
        database_hint.setText(
            "Database folder auto-detection has low confidence; verify selection."
            if suggestion.database.low_confidence
            else "Database folder auto-detected."
        )

        form = QFormLayout()
        form.addRow("Provider name", self._provider_name)
        form.addRow("Provider prefix", self._prefix)
        form.addRow("Repo URL", self._repo_url)
        form.addRow("Local checkout", self._repo_path)
        form.addRow("Symbols folder", self._symbols)
        form.addRow("", symbols_hint)
        form.addRow("Footprints folder", self._footprints)
        form.addRow("", footprints_hint)
        form.addRow("3D models folder", self._models)
        form.addRow("", models_hint)
        form.addRow("Design blocks folder", self._design_blocks)
        form.addRow("", design_blocks_hint)
        form.addRow("Database folder", self._database)
        form.addRow("", database_hint)

        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel)
        buttons.accepted.connect(self.accept)
        buttons.rejected.connect(self.reject)

        layout = QVBoxLayout(self)
        layout.addWidget(QLabel("Review the auto-detected folders and adjust if needed.", self))
        layout.addLayout(form)
        layout.addWidget(buttons)

    @property
    def provider_name(self) -> str:
        return self._provider_name.text().strip()

    @property
    def repo_path(self) -> str:
        return self._repo_path.text().strip()

    @property
    def provider_prefix(self) -> str:
        return self._prefix.text().strip().upper()

    @property
    def mapping(self) -> dict[str, str]:
        return {
            "symbols_path": self._symbols.currentText().strip(),
            "footprints_path": self._footprints.currentText().strip(),
            "models3d_path": self._models.currentText().strip(),
            "design_blocks_path": self._design_blocks.currentText().strip(),
            "database_path": self._database.currentText().strip(),
        }

    @property
    def provider_id(self) -> str:
        return sanitize_provider_id(self.provider_name)

    def accept(self) -> None:  # type: ignore[override]
        prefix = self.provider_prefix
        if not re.fullmatch(r"[A-Z]{2,3}", prefix):
            QMessageBox.warning(self, "Invalid prefix", "Provider prefix must be 2-3 uppercase letters.")
            return
        super().accept()


class ProviderManagerDialog(QDialog):
    def __init__(self, providers: list[tuple[str, str, str, str]], parent=None):
        super().__init__(parent)
        self.setWindowTitle("Library Providers")
        self._providers = providers
        self._list = QListWidget(self)
        for provider_id, name, repo_url, auth in providers:
            self._list.addItem(f"{name} [{provider_id}]  {auth or 'unverified'}  {repo_url}")

        add_btn = QPushButton("Add Provider", self)
        remove_btn = QPushButton("Remove Selected", self)
        add_btn.clicked.connect(self.accept)
        remove_btn.clicked.connect(self._remove_selected)

        row = QHBoxLayout()
        row.addWidget(add_btn)
        row.addWidget(remove_btn)

        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
        buttons.rejected.connect(self.reject)
        buttons.accepted.connect(self.reject)

        layout = QVBoxLayout(self)
        layout.addWidget(QLabel("Add or remove provider mappings.", self))
        layout.addWidget(self._list, 1)
        layout.addLayout(row)
        layout.addWidget(buttons)

        self.removed_provider_ids: list[str] = []
        self.add_new = False

    def _remove_selected(self) -> None:
        row = self._list.currentRow()
        if row < 0:
            QMessageBox.information(self, "Remove Provider", "Select a provider first.")
            return
        provider_id = self._providers[row][0]
        self.removed_provider_ids.append(provider_id)
        self._providers.pop(row)
        self._list.takeItem(row)

    def accept(self) -> None:  # type: ignore[override]
        self.add_new = True
        super().accept()


class SharePartsDialog(QDialog):
    def __init__(self, providers: list[tuple[str, str, str]], categories: list[str], parent=None):
        super().__init__(parent)
        self.setWindowTitle("Share Parts Across Providers")
        self._providers = providers

        self._category = QComboBox(self)
        self._category.addItems(categories)

        self._source = QComboBox(self)
        self._destination = QComboBox(self)
        for provider_id, name, prefix in providers:
            label = f"{prefix} - {name} [{provider_id}]"
            self._source.addItem(label, provider_id)
            self._destination.addItem(label, provider_id)

        self._ipns = QLineEdit(self)
        self._ipns.setPlaceholderText("Optional: comma-separated IPNs (leave empty to share all in category)")

        form = QFormLayout()
        form.addRow("Category", self._category)
        form.addRow("Source provider", self._source)
        form.addRow("Destination provider", self._destination)
        form.addRow("IPNs", self._ipns)

        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel)
        buttons.accepted.connect(self.accept)
        buttons.rejected.connect(self.reject)

        layout = QVBoxLayout(self)
        layout.addWidget(QLabel("Copy parts from one provider to another.", self))
        layout.addLayout(form)
        layout.addWidget(buttons)

    @property
    def category(self) -> str:
        return self._category.currentText().strip().lower()

    @property
    def source_provider_id(self) -> str:
        return str(self._source.currentData())

    @property
    def destination_provider_id(self) -> str:
        return str(self._destination.currentData())

    @property
    def ipns(self) -> set[str]:
        raw = self._ipns.text().strip()
        if not raw:
            return set()
        return {item.strip() for item in raw.split(",") if item.strip()}

    def accept(self) -> None:  # type: ignore[override]
        if self.source_provider_id == self.destination_provider_id:
            QMessageBox.warning(self, "Invalid selection", "Choose different source and destination providers.")
            return
        super().accept()

