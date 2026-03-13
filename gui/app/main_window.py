from __future__ import annotations

from datetime import datetime
from dataclasses import dataclass
from pathlib import Path
from PyQt6.QtCore import Qt, QUrl
from PyQt6.QtGui import QAction, QDesktopServices, QKeySequence, QUndoCommand, QUndoStack
from PyQt6.QtWidgets import (
    QAbstractItemView,
    QComboBox,
    QCompleter,
    QDockWidget,
    QFileDialog,
    QHBoxLayout,
    QInputDialog,
    QLabel,
    QLineEdit,
    QListWidget,
    QListWidgetItem,
    QMainWindow,
    QMessageBox,
    QProgressDialog,
    QPushButton,
    QSplitter,
    QStyledItemDelegate,
    QTableWidget,
    QTableWidgetItem,
    QToolBar,
    QVBoxLayout,
    QWidget,
)

from .bom_export import export_bom_long, export_bom_wide
from .copy_prompt_dialog import CopyPromptDialog
from .csv_manager import CsvDocument, read_csv, write_csv
from .csv_model import CsvTableModel
from .db_generator import generate_sqlite
from .dialogs import GenericAddPartDialog
from .error_handler import show_error_dialog
from .ipn import collect_all_ipns, generate_sequential_ipn
from .kicad_lib import KiCadLibraryIndex, reference_from_footprint_path, references_from_symbol_file
from .lib_sync import copy_footprint, copy_symbol
from .lib_viewer import FootprintViewer, SymbolViewer
from .schema import default_row_for_headers, discover_category_schemas
from .si_parser import parse_si_value
from .smart_form import SmartAddPartDialog
from .submodule_manager import SubmoduleWorker, submodule_heads
from .substitutes import SubstituteRecord, SubstitutesStore
from .supplier_api import SupplierApiClient
from .supplier_dialog import SupplierSearchDialog


OPEN_URL_COLUMNS = {"Datasheet", "LCSC"}
HEADER_DISPLAY_NAMES = {"DigiKey_PN": "DigiKey PN", "Mouser_PN": "Mouser PN"}
IPN_TOOLTIPS = {
    "res": "IPN format: RES-NNNN-VVVV where VVVV encodes the resistance value using E96 standard",
    "cap": "IPN format: CAP-NNNN-VVVV where VVVV encodes capacitance in pF notation",
    "ind": "IPN format: IND-NNNN-VVVV (sequential)",
}
CATEGORY_DESCRIPTIONS = {
    "ana": "Analog ICs: op-amps, comparators, ADC/DAC",
    "art": "Artwork items: fiducials, test points, graphics",
    "cap": "Capacitors",
    "con": "Connectors",
    "cpd": "Circuit protection devices",
    "dio": "Diodes",
    "ics": "Integrated circuits (general)",
    "ind": "Inductors and transformers",
    "mcu": "Microcontrollers and related modules",
    "mec": "Mechanical: screws, standoffs, spacers, etc.",
    "mpu": "Processors, SoMs, and SBCs",
    "opt": "Optoelectronics and optical components",
    "osc": "Oscillators and crystals",
    "pcb": "Printed circuit boards (bare boards)",
    "pwr": "Power components (relays, etc.)",
    "reg": "Voltage/current regulators",
    "res": "Resistors",
    "rfm": "RF modules and RF components",
    "rvr": "Variable resistors / trimmers / potentiometers",
    "swi": "Switches",
    "trs": "Transistors and FETs",
}


class CompleterDelegate(QStyledItemDelegate):
    def __init__(self, value_provider, readonly_provider, header_provider, browse_provider, parent=None):
        super().__init__(parent)
        self.value_provider = value_provider
        self.readonly_provider = readonly_provider
        self.header_provider = header_provider
        self.browse_provider = browse_provider

    def _apply_completer(self, editor: QLineEdit, column: int) -> None:
        values = self.value_provider(column)
        if values:
            completer = QCompleter(values, editor)
            completer.setCaseSensitivity(Qt.CaseSensitivity.CaseInsensitive)
            completer.setFilterMode(Qt.MatchFlag.MatchContains)
            editor.setCompleter(completer)

    def _editor_line_edit(self, editor):
        if isinstance(editor, QLineEdit):
            return editor
        return editor.findChild(QLineEdit)

    def createEditor(self, parent, option, index):  # type: ignore[override]
        if self.readonly_provider(index.column()):
            return None
        header = self.header_provider(index.column())
        if header in {"Symbol", "Footprint"}:
            editor_widget = QWidget(parent)
            row = QHBoxLayout(editor_widget)
            row.setContentsMargins(0, 0, 0, 0)
            row.setSpacing(4)
            editor = QLineEdit(editor_widget)
            self._apply_completer(editor, index.column())
            browse = QPushButton("...", editor_widget)
            browse.setFixedWidth(28)
            browse.clicked.connect(
                lambda: self._on_browse_clicked(header, editor, editor_widget)
            )
            row.addWidget(editor, 1)
            row.addWidget(browse)
            return editor_widget
        editor = QLineEdit(parent)
        self._apply_completer(editor, index.column())
        return editor

    def _on_browse_clicked(self, header: str, editor: QLineEdit, parent) -> None:
        picked = self.browse_provider(header, editor.text().strip(), parent)
        if picked:
            editor.setText(picked)

    def setEditorData(self, editor, index):  # type: ignore[override]
        line = self._editor_line_edit(editor)
        if line is None:
            return super().setEditorData(editor, index)
        value = index.model().data(index, Qt.ItemDataRole.EditRole) or ""
        line.setText(str(value))

    def setModelData(self, editor, model, index):  # type: ignore[override]
        line = self._editor_line_edit(editor)
        if line is None:
            return super().setModelData(editor, model, index)
        model.setData(index, line.text(), Qt.ItemDataRole.EditRole)


class SetCellCommand(QUndoCommand):
    def __init__(self, model: CsvTableModel, row: int, col: int, old: str, new: str):
        super().__init__("Edit cell")
        self.model = model
        self.row = row
        self.col = col
        self.old = old
        self.new = new

    def undo(self) -> None:
        self.model.set_cell(self.row, self.col, self.old)

    def redo(self) -> None:
        self.model.set_cell(self.row, self.col, self.new)


class InsertRowCommand(QUndoCommand):
    def __init__(self, model: CsvTableModel, row_data: dict[str, str]):
        super().__init__("Add row")
        self.model = model
        self.row_data = row_data
        self.row_idx = -1

    def undo(self) -> None:
        if self.row_idx >= 0:
            self.model.delete_row(self.row_idx)

    def redo(self) -> None:
        if self.row_idx < 0:
            self.row_idx = self.model.insert_row(self.row_data.copy())
        else:
            self.model.insert_row(self.row_data.copy())


class DeleteRowCommand(QUndoCommand):
    def __init__(self, model: CsvTableModel, row_idx: int):
        super().__init__("Delete row")
        self.model = model
        self.row_idx = row_idx
        self.cached_row: dict[str, str] | None = None

    def undo(self) -> None:
        if self.cached_row is not None:
            self.model.insert_row(self.cached_row.copy())

    def redo(self) -> None:
        self.cached_row = self.model.delete_row(self.row_idx)


@dataclass
class CategoryState:
    document: CsvDocument
    model: CsvTableModel
    dirty: bool = False


class MainWindow(QMainWindow):
    def __init__(self, workspace_root: Path):
        super().__init__()
        self.workspace_root = workspace_root
        self.database_dir = workspace_root / "database"
        self.setWindowTitle("KiCad Parts Manager GUI")
        self.resize(1400, 900)

        self.undo_stack = QUndoStack(self)
        self.schemas = discover_category_schemas(self.database_dir)
        self.categories: dict[str, CategoryState] = {}
        self.current_category = ""
        self._submodule_worker: SubmoduleWorker | None = None
        self._copy_guard = False

        self.substitutes = SubstitutesStore(self.database_dir / "substitutes.csv")
        self.supplier_api = SupplierApiClient(self.workspace_root / "secrets.env")
        self.library_index = KiCadLibraryIndex(self.workspace_root)

        self._build_ui()
        self._load_categories()
        self._setup_actions()
        self._start_submodule_task("ensure")

    def _build_ui(self) -> None:
        self.toolbar = QToolBar("Main")
        self.addToolBar(self.toolbar)

        self.sidebar = QListWidget()
        self.sidebar.currentItemChanged.connect(self._on_category_change)

        self.table = QTableWidget()
        self.table.setSortingEnabled(True)
        self.table.setEditTriggers(
            QAbstractItemView.EditTrigger.DoubleClicked | QAbstractItemView.EditTrigger.EditKeyPressed
        )
        self.table.setItemDelegate(
            CompleterDelegate(
                self._column_completer_values,
                self._is_column_read_only,
                self._column_header_name,
                self._browse_reference_for_column,
                self.table,
            )
        )
        self.table.itemChanged.connect(self._on_table_item_changed)
        self.table.itemDoubleClicked.connect(self._on_item_double_clicked)
        self.table.itemSelectionChanged.connect(self._on_selection_changed)

        self.sub_table = QTableWidget(0, 6)
        self.sub_table.setHorizontalHeaderLabels(["IPN", "MPN", "Manufacturer", "Datasheet", "Supplier", "SupplierPN"])

        add_sub_btn = QPushButton("Add Substitute")
        add_sub_btn.clicked.connect(self._add_substitute)
        find_alt_btn = QPushButton("Find Alternates")
        find_alt_btn.clicked.connect(self._find_alternates)
        sub_header = QHBoxLayout()
        sub_header.addWidget(QLabel("Substitutes for selected IPN"))
        sub_header.addStretch(1)
        sub_header.addWidget(find_alt_btn)
        sub_header.addWidget(add_sub_btn)

        center = QWidget()
        center_layout = QVBoxLayout(center)
        center_layout.addWidget(self.table, 4)
        center_layout.addLayout(sub_header)
        center_layout.addWidget(self.sub_table, 2)

        splitter = QSplitter()
        splitter.addWidget(self.sidebar)
        splitter.addWidget(center)
        splitter.setStretchFactor(1, 1)
        self.setCentralWidget(splitter)

        self.symbol_viewer = SymbolViewer(self.workspace_root, self)
        self.footprint_viewer = FootprintViewer(self.workspace_root, self)
        preview = QWidget()
        preview_layout = QVBoxLayout(preview)
        self.symbol_unit_label = QLabel("Unit")
        self.symbol_unit_combo = QComboBox()
        self.symbol_unit_combo.currentIndexChanged.connect(self._on_symbol_unit_changed)
        unit_row = QHBoxLayout()
        unit_row.addWidget(self.symbol_unit_label)
        unit_row.addWidget(self.symbol_unit_combo, 1)
        preview_layout.addLayout(unit_row)
        preview_layout.addWidget(self.symbol_viewer, 1)
        preview_layout.addWidget(self.footprint_viewer, 1)
        self.preview_dock = QDockWidget("Library Preview", self)
        self.preview_dock.setWidget(preview)
        self.addDockWidget(Qt.DockWidgetArea.RightDockWidgetArea, self.preview_dock)
        self._refresh_symbol_unit_picker()

        self.status = self.statusBar()
        self.libs_indicator = QLabel("KiCad refs: pending")
        self.status.addPermanentWidget(self.libs_indicator)
        self.status.showMessage("Ready")

    def _setup_actions(self) -> None:
        add_action = QAction("Add Part", self)
        add_action.triggered.connect(self.add_part)
        self.toolbar.addAction(add_action)

        save_action = QAction("Save", self)
        save_action.setShortcut(QKeySequence.StandardKey.Save)
        save_action.triggered.connect(self.save_current)
        self.toolbar.addAction(save_action)

        gen_db_action = QAction("Generate DB", self)
        gen_db_action.triggered.connect(self.generate_db)
        self.toolbar.addAction(gen_db_action)

        update_libs_action = QAction("Update Libraries", self)
        update_libs_action.triggered.connect(lambda: self._start_submodule_task("update"))
        self.toolbar.addAction(update_libs_action)

        search_action = QAction("Search", self)
        search_action.setShortcut(QKeySequence.StandardKey.Find)
        search_action.triggered.connect(self.search_all)
        self.toolbar.addAction(search_action)

        filter_action = QAction("Filter Column", self)
        filter_action.triggered.connect(self.filter_column)
        self.toolbar.addAction(filter_action)

        dup_action = QAction("Duplicate Row", self)
        dup_action.triggered.connect(self.duplicate_selected_row)
        self.toolbar.addAction(dup_action)

        del_action = QAction("Delete Row", self)
        del_action.triggered.connect(self.delete_selected_row)
        self.toolbar.addAction(del_action)

        export_action = QAction("Export BOM", self)
        export_action.triggered.connect(self.export_bom)
        self.toolbar.addAction(export_action)

        undo_action = self.undo_stack.createUndoAction(self, "Undo")
        undo_action.setShortcut(QKeySequence.StandardKey.Undo)
        redo_action = self.undo_stack.createRedoAction(self, "Redo")
        redo_action.setShortcut(QKeySequence.StandardKey.Redo)
        self.toolbar.addAction(undo_action)
        self.toolbar.addAction(redo_action)

    def _start_submodule_task(self, mode: str) -> None:
        if self._submodule_worker and self._submodule_worker.isRunning():
            return
        self.status.showMessage("Syncing KiCad reference libraries...")
        self.libs_indicator.setText("KiCad refs: syncing...")
        self._submodule_worker = SubmoduleWorker(self.workspace_root, mode=mode)
        self._submodule_worker.completed.connect(self._on_submodule_task_done)
        self._submodule_worker.start()

    def _on_submodule_task_done(self, ok: bool, output: str) -> None:
        if not ok:
            show_error_dialog(self, "Library sync failed", "Unable to sync KiCad submodules", output)
            self.status.showMessage("Library sync failed")
            self.libs_indicator.setText("KiCad refs: sync failed")
            return
        self.library_index.rebuild()
        heads = submodule_heads(self.workspace_root)
        short = (
            f"S:{heads.get('libs/kicad-symbols', 'n/a')} "
            f"F:{heads.get('libs/kicad-footprints', 'n/a')} "
            f"U:{heads.get('libs/kicad-library-utils', 'n/a')}"
        )
        stamp = datetime.now().strftime("%Y-%m-%d %H:%M")
        self.libs_indicator.setText(f"KiCad refs [{short}] @ {stamp}")
        self.status.showMessage("KiCad reference libraries ready")

    def _column_completer_values(self, column_index: int) -> list[str]:
        if not self.current_category:
            return []
        headers = self.categories[self.current_category].model.headers
        if column_index < 0 or column_index >= len(headers):
            return []
        header = headers[column_index]
        if header == "Footprint":
            return [entry.name for entry in self.library_index.entries("footprint")]
        if header == "Symbol":
            return [entry.name for entry in self.library_index.entries("symbol")]
        return []

    def _column_header_name(self, column_index: int) -> str:
        if not self.current_category:
            return ""
        headers = self.categories[self.current_category].model.headers
        if column_index < 0 or column_index >= len(headers):
            return ""
        return headers[column_index]

    def _browse_reference_for_column(self, header: str, current_value: str, parent) -> str | None:
        if header not in {"Symbol", "Footprint"}:
            return None
        start_dir = self.workspace_root / ("symbols" if header == "Symbol" else "footprints")
        if ":" in current_value:
            kind = "symbol" if header == "Symbol" else "footprint"
            entry = self.library_index.resolve(current_value, kind)
            if entry and entry.file_path.exists():
                start_dir = entry.file_path.parent
        if not start_dir.exists():
            start_dir = self.workspace_root / "libs" / ("kicad-symbols" if header == "Symbol" else "kicad-footprints")

        if header == "Symbol":
            selected, _ = QFileDialog.getOpenFileName(
                parent,
                "Select Symbol Library",
                str(start_dir),
                "KiCad Symbol (*.kicad_sym)",
            )
            if not selected:
                return None
            refs = references_from_symbol_file(Path(selected))
            if not refs:
                return None
            selected_ref = refs[0]
            if len(refs) > 1:
                preferred = current_value.split(":", 1)[1] if ":" in current_value else ""
                preferred_index = 0
                if preferred:
                    for idx, ref in enumerate(refs):
                        if ref.endswith(f":{preferred}"):
                            preferred_index = idx
                            break
                selected_ref, ok = QInputDialog.getItem(parent, "Select Symbol", "Symbol", refs, preferred_index, False)
                if not ok:
                    return None
            return self._maybe_copy_external_reference("Symbol", selected_ref)

        selected, _ = QFileDialog.getOpenFileName(
            parent,
            "Select Footprint",
            str(start_dir),
            "KiCad Footprint (*.kicad_mod)",
        )
        if not selected:
            return None
        ref = reference_from_footprint_path(Path(selected))
        if not ref:
            return None
        return self._maybe_copy_external_reference("Footprint", ref)

    def _is_column_read_only(self, column_index: int) -> bool:
        if not self.current_category:
            return False
        headers = self.categories[self.current_category].model.headers
        if column_index < 0 or column_index >= len(headers):
            return False
        return headers[column_index] == "IPN" and self.current_category in {"res", "cap", "ind"}

    def _load_categories(self) -> None:
        for schema in self.schemas:
            doc = read_csv(schema.csv_path)
            model = CsvTableModel(doc.headers, doc.rows)
            model.dirtyChanged.connect(lambda dirty, key=schema.key: self._set_dirty(key, dirty))
            self.categories[schema.key] = CategoryState(document=doc, model=model)

            item = QListWidgetItem(self._category_label(schema.key, dirty=False))
            item.setData(Qt.ItemDataRole.UserRole, schema.key)
            item.setToolTip(self._category_tooltip(schema.key))
            self.sidebar.addItem(item)
        if self.sidebar.count():
            self.sidebar.setCurrentRow(0)

    def _category_label(self, key: str, dirty: bool) -> str:
        base = key.upper()
        return f"{base} *" if dirty else base

    def _category_tooltip(self, key: str) -> str:
        code = key.upper()
        desc = CATEGORY_DESCRIPTIONS.get(key.lower(), "Category")
        return f"{code}: {desc}"

    def _set_dirty(self, category: str, dirty: bool) -> None:
        state = self.categories[category]
        state.dirty = dirty
        for row in range(self.sidebar.count()):
            item = self.sidebar.item(row)
            key = item.data(Qt.ItemDataRole.UserRole)
            if key == category:
                item.setText(self._category_label(key, dirty))
                item.setToolTip(self._category_tooltip(key))
                break
        self._update_status()

    def _update_status(self) -> None:
        dirty_count = sum(1 for state in self.categories.values() if state.dirty)
        self.status.showMessage(f"{dirty_count} dirty categories")

    def _on_category_change(self, current: QListWidgetItem | None, _prev: QListWidgetItem | None) -> None:
        if not current:
            return
        self.current_category = current.data(Qt.ItemDataRole.UserRole)
        self._render_current_table()

    def _render_current_table(self) -> None:
        state = self.categories[self.current_category]
        model = state.model
        self.table.blockSignals(True)
        self.table.clear()
        self.table.setRowCount(model.rowCount())
        self.table.setColumnCount(model.columnCount())
        display_headers = [HEADER_DISPLAY_NAMES.get(header, header) for header in model.headers]
        self.table.setHorizontalHeaderLabels(display_headers)
        if "IPN" in model.headers:
            ipn_col = model.headers.index("IPN")
            ipn_item = self.table.horizontalHeaderItem(ipn_col)
            if ipn_item:
                ipn_item.setToolTip(
                    IPN_TOOLTIPS.get(
                        self.current_category,
                        "IPN format: CCC-NNNN-VVVV (sequential assignment)",
                    )
                )
        for row_idx in range(model.rowCount()):
            for col_idx, header in enumerate(model.headers):
                item = QTableWidgetItem(model.rows[row_idx].get(header, ""))
                if header in OPEN_URL_COLUMNS:
                    item.setForeground(Qt.GlobalColor.blue)
                self.table.setItem(row_idx, col_idx, item)
        self.table.blockSignals(False)
        if model.rowCount() > 0:
            self.table.setCurrentCell(0, 0)
            self._update_preview_for_row(0)
        self._refresh_substitutes_panel()

    def _on_table_item_changed(self, item: QTableWidgetItem) -> None:
        if not self.current_category:
            return
        if self._copy_guard:
            return
        state = self.categories[self.current_category]
        model = state.model
        row, col = item.row(), item.column()
        header = model.headers[col]
        old = model.rows[row].get(header, "")
        new = item.text()
        if header in {"Symbol", "Footprint"}:
            new = self._maybe_copy_external_reference(header, new)
            if new != item.text():
                self._copy_guard = True
                try:
                    item.setText(new)
                finally:
                    self._copy_guard = False
        if old == new:
            return
        self.undo_stack.push(SetCellCommand(model, row, col, old, new))
        self._render_current_table()

    def _on_item_double_clicked(self, item: QTableWidgetItem) -> None:
        if not self.current_category:
            return
        header = self.categories[self.current_category].model.headers[item.column()]
        if header in {"Symbol", "Footprint"}:
            self._update_preview_for_value(header, item.text().strip())
            return
        if header not in OPEN_URL_COLUMNS:
            return
        url = item.text().strip()
        if url.startswith("http://") or url.startswith("https://"):
            QDesktopServices.openUrl(QUrl(url))

    def _on_selection_changed(self) -> None:
        row = self.table.currentRow()
        if row < 0 or not self.current_category:
            return
        self._update_preview_for_row(row)

    def _update_preview_for_row(self, row: int) -> None:
        if not self.current_category or row < 0:
            self.symbol_viewer.clear()
            self._refresh_symbol_unit_picker()
            self.footprint_viewer.clear()
            return
        headers = self.categories[self.current_category].model.headers
        sym_val = ""
        fp_val = ""
        if "Symbol" in headers:
            sym_item = self.table.item(row, headers.index("Symbol"))
            sym_val = sym_item.text().strip() if sym_item else ""
        if "Footprint" in headers:
            fp_item = self.table.item(row, headers.index("Footprint"))
            fp_val = fp_item.text().strip() if fp_item else ""
        try:
            self._update_preview_for_value("Symbol", sym_val)
        except Exception:
            self.symbol_viewer.clear()
            self._refresh_symbol_unit_picker()
        try:
            self._update_preview_for_value("Footprint", fp_val)
        except Exception:
            self.footprint_viewer.clear()

    def _update_preview_for_value(self, header: str, value: str) -> None:
        if header == "Footprint":
            if ":" not in value:
                self.footprint_viewer.clear()
                return
            entry = self.library_index.resolve(value, "footprint")
            if entry:
                self.footprint_viewer.load_file(entry.file_path)
            else:
                self.footprint_viewer.clear()
        elif header == "Symbol":
            if ":" not in value:
                self.symbol_viewer.clear()
                self._refresh_symbol_unit_picker()
                return
            entry = self.library_index.resolve(value, "symbol")
            if entry:
                symbol_name = entry.name.split(":", 1)[1]
                self.symbol_viewer.load_symbol(entry.file_path, symbol_name)
                self._refresh_symbol_unit_picker()
            else:
                self.symbol_viewer.clear()
                self._refresh_symbol_unit_picker()

    def _refresh_symbol_unit_picker(self) -> None:
        options = self.symbol_viewer.unit_options()
        selected_unit = self.symbol_viewer.selected_unit
        self.symbol_unit_combo.blockSignals(True)
        self.symbol_unit_combo.clear()
        selected_index = 0
        for index, (unit, label) in enumerate(options):
            self.symbol_unit_combo.addItem(label, unit)
            if unit == selected_unit:
                selected_index = index
        if options:
            self.symbol_unit_combo.setCurrentIndex(selected_index)
        self.symbol_unit_combo.blockSignals(False)
        show_picker = len(options) > 1
        self.symbol_unit_label.setVisible(show_picker)
        self.symbol_unit_combo.setVisible(show_picker)
        self.symbol_unit_combo.setEnabled(show_picker)

    def _on_symbol_unit_changed(self, index: int) -> None:
        unit = self.symbol_unit_combo.itemData(index)
        if unit is None:
            return
        self.symbol_viewer.set_selected_unit(int(unit))

    def _maybe_copy_external_reference(self, header: str, value: str) -> str:
        if ":" not in value:
            return value
        kind = "symbol" if header == "Symbol" else "footprint"
        entry = self.library_index.resolve(value, kind)
        if not entry or entry.source != "kicad":
            return value

        if kind == "symbol":
            local_libraries = self.library_index.local_symbol_libraries()
        else:
            local_libraries = self.library_index.local_footprint_libraries()
        if not local_libraries:
            return value

        preview_widget = None
        if kind == "symbol":
            preview_widget = SymbolViewer(self.workspace_root, self)
            preview_widget.load_symbol(entry.file_path, entry.name.split(":", 1)[1])
        else:
            preview_widget = FootprintViewer(self.workspace_root, self)
            preview_widget.load_file(entry.file_path)

        prompt = CopyPromptDialog(
            kind,
            entry.name,
            str(entry.file_path),
            local_libraries,
            preview_widget=preview_widget,
            parent=self,
        )
        if prompt.exec() != prompt.DialogCode.Accepted:
            return value
        target_lib = prompt.target_library.strip()
        if not target_lib:
            return value

        try:
            if kind == "symbol":
                symbol_name = entry.name.split(":", 1)[1]
                result = copy_symbol(
                    src_sym_path=entry.file_path,
                    symbol_name=symbol_name,
                    dest_sym_path=self.workspace_root / "symbols" / f"{target_lib}.kicad_sym",
                )
                if result.copied or "already exists" in result.message.lower():
                    self.library_index.rebuild()
                    return f"{target_lib}:{symbol_name}"
            else:
                result, _model_results = copy_footprint(
                    src_mod_path=entry.file_path,
                    dest_pretty_dir=self.workspace_root / "footprints" / f"{target_lib}.pretty",
                    local_3d_dir=self.workspace_root / "3d-models",
                )
                if result.copied or "already exists" in result.message.lower():
                    self.library_index.rebuild()
                    return f"{target_lib}:{entry.file_path.stem}"
        except Exception as exc:
            show_error_dialog(self, "Copy to local library failed", str(exc))
        return value

    def _selected_ipn(self) -> str:
        selected = self.table.currentRow()
        if selected < 0 or not self.current_category:
            return ""
        headers = self.categories[self.current_category].model.headers
        if "IPN" not in headers:
            return ""
        ipn_col = headers.index("IPN")
        item = self.table.item(selected, ipn_col)
        return item.text().strip() if item else ""

    def _refresh_substitutes_panel(self) -> None:
        ipn = self._selected_ipn()
        records = self.substitutes.by_ipn(ipn) if ipn else []
        self.sub_table.setRowCount(len(records))
        for row, record in enumerate(records):
            values = [record.ipn, record.mpn, record.manufacturer, record.datasheet, record.supplier, record.supplier_pn]
            for col, value in enumerate(values):
                self.sub_table.setItem(row, col, QTableWidgetItem(value))

    def _add_substitute(self) -> None:
        ipn = self._selected_ipn()
        if not ipn:
            QMessageBox.information(self, "No part selected", "Select a part row first.")
            return
        mpn, ok = QInputDialog.getText(self, "Add Substitute", "MPN:")
        if not ok or not mpn.strip():
            return
        mfg, _ = QInputDialog.getText(self, "Add Substitute", "Manufacturer:")
        datasheet, _ = QInputDialog.getText(self, "Add Substitute", "Datasheet URL:")
        supplier, _ = QInputDialog.getText(self, "Add Substitute", "Supplier:")
        supplier_pn, _ = QInputDialog.getText(self, "Add Substitute", "Supplier PN:")
        self.substitutes.add(
            SubstituteRecord(
                ipn=ipn,
                mpn=mpn.strip(),
                manufacturer=mfg.strip(),
                datasheet=datasheet.strip(),
                supplier=supplier.strip(),
                supplier_pn=supplier_pn.strip(),
            )
        )
        self._refresh_substitutes_panel()

    def _find_alternates(self) -> None:
        dialog = SupplierSearchDialog(self.supplier_api, self)
        ipn = self._selected_ipn()
        if ipn:
            dialog.query.setText(ipn)
        if dialog.exec() != dialog.DialogCode.Accepted or not dialog.selected_part:
            return
        part = dialog.selected_part
        self.substitutes.add(
            SubstituteRecord(
                ipn=ipn,
                mpn=part.mpn,
                manufacturer=part.manufacturer,
                datasheet=part.datasheet,
                supplier=part.source,
                supplier_pn=part.digikey_pn or part.mouser_pn,
            )
        )
        self._refresh_substitutes_panel()

    def _supplier_dialog_factory(self, parent):
        return SupplierSearchDialog(self.supplier_api, parent)

    def add_part(self) -> None:
        if not self.current_category:
            return
        state = self.categories[self.current_category]
        headers = state.model.headers
        existing = collect_all_ipns(self.database_dir)

        if self.current_category in {"res", "cap", "ind"}:
            dialog = SmartAddPartDialog(
                self.current_category,
                existing,
                self._supplier_dialog_factory,
                self.library_index,
                self.workspace_root,
                reference_handler=self._maybe_copy_external_reference,
                parent=self,
            )
            if dialog.exec() != dialog.DialogCode.Accepted:
                return
            row_data = default_row_for_headers(headers)
            row_data.update(dialog.row_data())
        else:
            ccc = self.current_category.upper()
            defaults = {"IPN": generate_sequential_ipn(ccc, existing)}
            completer_values = {
                "Symbol": [entry.name for entry in self.library_index.entries("symbol")],
                "Footprint": [entry.name for entry in self.library_index.entries("footprint")],
            }
            dialog = GenericAddPartDialog(
                headers,
                defaults=defaults,
                completer_values=completer_values,
                workspace_root=self.workspace_root,
                library_index=self.library_index,
                reference_handler=self._maybe_copy_external_reference,
                category=self.current_category,
                supplier_dialog_factory=self._supplier_dialog_factory,
                parent=self,
            )
            if dialog.exec() != dialog.DialogCode.Accepted:
                return
            row_data = dialog.row_data()
            if "Symbol" in row_data:
                row_data["Symbol"] = self._maybe_copy_external_reference("Symbol", row_data["Symbol"])
            if "Footprint" in row_data:
                row_data["Footprint"] = self._maybe_copy_external_reference("Footprint", row_data["Footprint"])

        self.undo_stack.push(InsertRowCommand(state.model, row_data))
        self._render_current_table()

    def save_current(self) -> None:
        if not self.current_category:
            return
        state = self.categories[self.current_category]
        state.document.rows = state.model.rows
        write_csv(state.document, make_backup=True)
        state.model.set_dirty(False)
        self._render_current_table()

    def search_all(self) -> None:
        query, ok = QInputDialog.getText(self, "Search", "Search IPN / MPN / Description / Value")
        if not ok or not query.strip():
            return
        lines: list[str] = []
        query_text = query.strip()
        needle = query_text.lower()
        parsed_query = None
        try:
            parsed_query = parse_si_value(query_text)
        except Exception:
            parsed_query = None
        for category, state in self.categories.items():
            for row in state.model.rows:
                value_text = row.get("Value", "")
                hay = " ".join((row.get("IPN", ""), row.get("MPN", ""), row.get("Description", ""), value_text)).lower()
                if needle in hay or (
                    parsed_query is not None and value_text and self._si_values_equivalent(parsed_query, value_text)
                ):
                    lines.append(f"{category}: {row.get('IPN', '')} | {row.get('MPN', '')} | {row.get('Description', '')}")
        if not lines:
            QMessageBox.information(self, "Search", "No matches")
            return
        QMessageBox.information(self, "Search results", "\n".join(lines[:200]))

    def filter_column(self) -> None:
        if not self.current_category:
            return
        state = self.categories[self.current_category]
        headers = state.model.headers
        column_name, ok = QInputDialog.getItem(self, "Filter", "Column", headers, 0, False)
        if not ok:
            return
        text, ok = QInputDialog.getText(self, "Filter", f"Contains value in {column_name}")
        if not ok:
            return
        col_idx = headers.index(column_name)
        needle = text.strip().lower()
        parsed_filter = None
        if column_name == "Value" and text.strip():
            try:
                parsed_filter = parse_si_value(text.strip())
            except Exception:
                parsed_filter = None
        for row in range(self.table.rowCount()):
            item = self.table.item(row, col_idx)
            hay = (item.text() if item else "").lower()
            matches = not bool(needle) or needle in hay
            if not matches and parsed_filter is not None:
                matches = self._si_values_equivalent(parsed_filter, item.text() if item else "")
            self.table.setRowHidden(row, not matches)

    def _si_values_equivalent(self, query_value: float, candidate_text: str) -> bool:
        try:
            candidate_value = parse_si_value(candidate_text)
        except Exception:
            return False
        if query_value == 0 or candidate_value == 0:
            return abs(query_value - candidate_value) <= 1e-18
        return abs(query_value - candidate_value) / max(abs(query_value), abs(candidate_value)) <= 0.001

    def delete_selected_row(self) -> None:
        if not self.current_category:
            return
        row = self.table.currentRow()
        if row < 0:
            return
        state = self.categories[self.current_category]
        self.undo_stack.push(DeleteRowCommand(state.model, row))
        self._render_current_table()

    def duplicate_selected_row(self) -> None:
        if not self.current_category:
            return
        row = self.table.currentRow()
        if row < 0:
            return
        state = self.categories[self.current_category]
        row_data = state.model.rows[row].copy()
        self.undo_stack.push(InsertRowCommand(state.model, row_data))
        self._render_current_table()

    def generate_db(self) -> None:
        progress = QProgressDialog("Generating database...", "Cancel", 0, len(self.schemas), self)
        progress.setWindowModality(Qt.WindowModality.WindowModal)
        out_db = self.database_dir / "parts.sqlite"

        def on_progress(idx: int, total: int, table: str) -> None:
            progress.setMaximum(total)
            progress.setValue(idx)
            progress.setLabelText(f"Importing {table} ({idx}/{total})")

        generate_sqlite(self.database_dir, out_db, progress_cb=on_progress)
        progress.setValue(len(self.schemas))
        QMessageBox.information(self, "Done", f"Generated {out_db}")

    def export_bom(self) -> None:
        if not self.current_category:
            return
        state = self.categories[self.current_category]
        out_path, _ = QFileDialog.getSaveFileName(self, "Export BOM", str(self.workspace_root), "CSV files (*.csv)")
        if not out_path:
            return
        mode, ok = QInputDialog.getItem(self, "BOM format", "Choose format", ["Wide", "Long"], 0, False)
        if not ok:
            return
        if mode == "Wide":
            export_bom_wide(state.model.rows, Path(out_path), self.substitutes)
        else:
            export_bom_long(state.model.rows, Path(out_path), self.substitutes)
        QMessageBox.information(self, "Export complete", out_path)

    def has_unsaved_changes(self) -> bool:
        return any(state.dirty for state in self.categories.values())

    def closeEvent(self, event) -> None:  # type: ignore[override]
        if not self.has_unsaved_changes():
            event.accept()
            return
        result = QMessageBox.question(
            self,
            "Unsaved changes",
            "You have unsaved changes. Save before closing?",
            QMessageBox.StandardButton.Save | QMessageBox.StandardButton.Discard | QMessageBox.StandardButton.Cancel,
            QMessageBox.StandardButton.Save,
        )
        if result == QMessageBox.StandardButton.Cancel:
            event.ignore()
            return
        if result == QMessageBox.StandardButton.Save:
            for key in list(self.categories.keys()):
                self.current_category = key
                self.save_current()
        event.accept()

