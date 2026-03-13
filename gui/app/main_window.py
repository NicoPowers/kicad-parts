from __future__ import annotations

from datetime import datetime, timedelta, timezone
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import quote
from PyQt6.QtCore import Qt, QUrl
from PyQt6.QtGui import QAction, QColor, QDesktopServices, QKeySequence, QUndoCommand, QUndoStack
from PyQt6.QtWidgets import (
    QAbstractItemView,
    QApplication,
    QComboBox,
    QCompleter,
    QDockWidget,
    QFileDialog,
    QHeaderView,
    QHBoxLayout,
    QInputDialog,
    QLabel,
    QLineEdit,
    QListWidget,
    QListWidgetItem,
    QMainWindow,
    QMenu,
    QMessageBox,
    QProgressBar,
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
from .search import search_local_inventory
from .si_parser import parse_si_value
from .smart_form import SmartAddPartDialog
from .submodule_manager import SubmoduleWorker, submodule_heads
from .substitutes import SubstituteRecord, SubstitutesStore
from .supplier_api import SupplierApiClient
from .supplier_dialog import (
    PnAssignProgressDialog,
    PriceSyncProgressDialog,
    PriceSyncWorker,
    SupplierSearchDialog,
)


OPEN_URL_COLUMNS = {"Datasheet", "LCSC"}
HEADER_DISPLAY_NAMES = {
    "DigiKey_PN": "DigiKey PN",
    "Mouser_PN": "Mouser PN",
    "DigiKey_Price": "DigiKey Price",
    "Mouser_Price": "Mouser Price",
    "Price_Range": "Price Range",
    "Price_LastSynced_UTC": "Price Last Synced (UTC)",
}
PRICE_COLUMNS = ("DigiKey_Price", "Mouser_Price", "Price_Range", "Price_LastSynced_UTC")
HIDDEN_UI_COLUMNS = {"DigiKey_Price", "Mouser_Price", "Price_LastSynced_UTC"}
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
            editor_widget.setAutoFillBackground(True)
            editor_widget.setAttribute(Qt.WidgetAttribute.WA_OpaquePaintEvent, True)
            editor_widget.setStyleSheet("background-color: palette(base);")
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

    def updateEditorGeometry(self, editor, option, index):  # type: ignore[override]
        editor.setGeometry(option.rect)

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
        self._price_sync_worker: PriceSyncWorker | None = None
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
        self.table.setSelectionMode(QAbstractItemView.SelectionMode.ExtendedSelection)
        self.table.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
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
        self.table.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self.table.customContextMenuRequested.connect(self._open_main_table_context_menu)

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
        save_action.triggered.connect(self.save_all)
        self.toolbar.addAction(save_action)

        gen_db_action = QAction("Generate DB", self)
        gen_db_action.triggered.connect(self.generate_db)
        self.toolbar.addAction(gen_db_action)

        update_libs_action = QAction("Update Libraries", self)
        update_libs_action.triggered.connect(lambda: self._start_submodule_task("update"))
        self.toolbar.addAction(update_libs_action)

        search_action = QAction("Search", self)
        search_action.setShortcut(QKeySequence.StandardKey.Find)
        search_action.setShortcutContext(Qt.ShortcutContext.ApplicationShortcut)
        search_action.triggered.connect(self._handle_find_shortcut)
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
        self.undo_stack.indexChanged.connect(self._on_undo_index_changed)

    def _on_undo_index_changed(self) -> None:
        if self.current_category:
            self._render_current_table()

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
        start_path = self.workspace_root / ("symbols" if header == "Symbol" else "footprints")
        if ":" in current_value:
            kind = "symbol" if header == "Symbol" else "footprint"
            entry = self.library_index.resolve(current_value, kind)
            if entry and entry.file_path.exists():
                start_path = entry.file_path
        if not start_path.exists():
            start_path = self.workspace_root / "libs" / ("kicad-symbols" if header == "Symbol" else "kicad-footprints")

        if header == "Symbol":
            selected, _ = QFileDialog.getOpenFileName(
                parent,
                "Select Symbol Library",
                str(start_path),
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
            str(start_path),
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
            self._ensure_price_columns(doc)
            model = CsvTableModel(doc.headers, doc.rows)
            model.dirtyChanged.connect(lambda dirty, key=schema.key: self._set_dirty(key, dirty))
            self.categories[schema.key] = CategoryState(document=doc, model=model)

            item = QListWidgetItem(self._category_label(schema.key, dirty=False))
            item.setData(Qt.ItemDataRole.UserRole, schema.key)
            item.setToolTip(self._category_tooltip(schema.key))
            self.sidebar.addItem(item)
        if self.sidebar.count():
            self.sidebar.setCurrentRow(0)

    def _ensure_price_columns(self, document: CsvDocument) -> None:
        changed = False
        for column in PRICE_COLUMNS:
            if column not in document.headers:
                document.headers.append(column)
                changed = True
        if not changed:
            return
        for row in document.rows:
            for column in PRICE_COLUMNS:
                row.setdefault(column, "")
            if not row.get("Price_Range", "").strip():
                row["Price_Range"] = "?"

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
                if header == "Price_Range":
                    self._decorate_price_range_item(item, model.rows[row_idx])
                elif header in {"Symbol", "Footprint"}:
                    self._decorate_library_reference_item(item, header, model.rows[row_idx].get(header, ""))
                self.table.setItem(row_idx, col_idx, item)
        header_view = self.table.horizontalHeader()
        for col_idx, header in enumerate(model.headers):
            if header in {"Symbol", "Footprint"}:
                header_view.setSectionResizeMode(col_idx, QHeaderView.ResizeMode.ResizeToContents)
            self.table.setColumnHidden(col_idx, header in HIDDEN_UI_COLUMNS)
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
        if header == "Price_Range":
            self._start_async_price_sync(item)
            return
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
                result, model_results = copy_footprint(
                    src_mod_path=entry.file_path,
                    dest_pretty_dir=self.workspace_root / "footprints" / f"{target_lib}.pretty",
                    local_3d_dir=self.workspace_root / "3d-models",
                    workspace_root=self.workspace_root,
                )
                if result.copied or "already exists" in result.message.lower():
                    failed_models = [r for r in model_results if not r.copied and r.message and "already exists" not in r.message.lower()]
                    if failed_models:
                        msgs = "\n".join(r.message for r in failed_models)
                        QMessageBox.warning(self, "3D model(s) unavailable", msgs)
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

    def _selected_rows(self) -> list[int]:
        selection_model = self.table.selectionModel()
        if selection_model:
            rows = sorted({idx.row() for idx in selection_model.selectedRows()})
            if rows:
                return rows
            rows = sorted({idx.row() for idx in selection_model.selectedIndexes()})
            if rows:
                return rows
        current = self.table.currentRow()
        return [current] if current >= 0 else []

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

    def _active_supplier_dialog(self) -> SupplierSearchDialog | None:
        active = QApplication.activeModalWidget() or QApplication.activeWindow()
        if isinstance(active, SupplierSearchDialog):
            return active
        for widget in QApplication.topLevelWidgets():
            if isinstance(widget, SupplierSearchDialog) and widget.isVisible() and widget.isActiveWindow():
                return widget
        return None

    def _handle_find_shortcut(self) -> None:
        active_supplier = self._active_supplier_dialog()
        if active_supplier is not None:
            active_supplier.focus_query()
            return
        self.search_all()

    def _local_search_summary(self, query: str):
        categories = {category: state.model.rows for category, state in self.categories.items()}
        return search_local_inventory(categories, query, limit=60)

    def _find_row_by_ipn(self, category: str, ipn: str) -> int:
        state = self.categories.get(category)
        if state is None:
            return -1
        for idx, row in enumerate(state.model.rows):
            if row.get("IPN", "").strip() == ipn:
                return idx
        return -1

    def _apply_row_updates(
        self,
        category: str,
        ipn: str,
        updates: dict[str, str],
        *,
        render: bool = True,
    ) -> dict[str, str] | None:
        state = self.categories.get(category)
        if state is None:
            return None
        row_idx = self._find_row_by_ipn(category, ipn)
        if row_idx < 0:
            return None
        model = state.model
        changed_pairs: list[tuple[int, str, str]] = []
        for header, new_value in updates.items():
            if header not in model.headers:
                continue
            col = model.headers.index(header)
            old_value = model.rows[row_idx].get(header, "")
            if old_value == new_value:
                continue
            changed_pairs.append((col, old_value, new_value))
        if not changed_pairs:
            return model.rows[row_idx].copy()
        self.undo_stack.beginMacro("Update local part from search")
        try:
            for col, old_value, new_value in changed_pairs:
                self.undo_stack.push(SetCellCommand(model, row_idx, col, old_value, new_value))
        finally:
            self.undo_stack.endMacro()
        if render and self.current_category == category:
            self._render_current_table()
        return model.rows[row_idx].copy()

    def _apply_supplier_assignment_row(self, category: str, ipn: str, updates: dict[str, str]) -> bool:
        return self._apply_row_updates(category, ipn, updates, render=False) is not None

    def _sync_local_price_range(self, category: str, ipn: str) -> dict[str, str] | None:
        state = self.categories.get(category)
        if state is None:
            return None
        row_idx = self._find_row_by_ipn(category, ipn)
        if row_idx < 0:
            return None
        row = state.model.rows[row_idx]
        digikey_pn = row.get("DigiKey_PN", "").strip()
        mouser_pn = row.get("Mouser_PN", "").strip()
        mpn_hint = row.get("MPN", "").strip()
        if not digikey_pn and not mouser_pn:
            updates = {
                "DigiKey_Price": "",
                "Mouser_Price": "",
                "Price_Range": "?",
                "Price_LastSynced_UTC": "",
            }
            return self._apply_row_updates(category, ipn, updates)
        prices = self.supplier_api.fetch_supplier_prices(digikey_pn, mouser_pn, mpn_hint=mpn_hint)
        updates = {
            "DigiKey_Price": prices.get("DigiKey_Price", ""),
            "Mouser_Price": prices.get("Mouser_Price", ""),
            "Price_Range": prices.get("Price_Range", "?") or "?",
            "Price_LastSynced_UTC": datetime.now(timezone.utc).isoformat(),
        }
        return self._apply_row_updates(category, ipn, updates)

    def _start_async_price_sync(self, table_item: QTableWidgetItem) -> None:
        if not self.current_category:
            return
        ipn = self._selected_ipn()
        if not ipn:
            return
        state = self.categories.get(self.current_category)
        if state is None:
            return
        row_idx = self._find_row_by_ipn(self.current_category, ipn)
        if row_idx < 0:
            return
        row = state.model.rows[row_idx]
        dk_pn = row.get("DigiKey_PN", "").strip()
        mouser_pn = row.get("Mouser_PN", "").strip()
        if not dk_pn and not mouser_pn:
            return
        if self._price_sync_worker and self._price_sync_worker.isRunning():
            return
        loading = QProgressBar()
        loading.setRange(0, 0)
        loading.setTextVisible(False)
        loading.setMaximumHeight(20)
        self.table.setCellWidget(table_item.row(), table_item.column(), loading)

        self._price_sync_worker = PriceSyncWorker(
            self.supplier_api, dk_pn, mouser_pn,
            row.get("MPN", "").strip(),
            self.current_category, ipn,
        )
        self._price_sync_worker.finished_prices.connect(self._on_main_price_sync_done)
        self._price_sync_worker.failed.connect(self._on_main_price_sync_failed)
        self._price_sync_worker.start()

    def _on_main_price_sync_done(self, prices: dict) -> None:
        category = prices.pop("_category", "")
        ipn = prices.pop("_ipn", "")
        timestamp = prices.pop("_timestamp", "")
        updates = {
            "DigiKey_Price": prices.get("DigiKey_Price", ""),
            "Mouser_Price": prices.get("Mouser_Price", ""),
            "Price_Range": prices.get("Price_Range", "?") or "?",
            "Price_LastSynced_UTC": timestamp,
        }
        self._apply_row_updates(category, ipn, updates)
        if self.current_category == category:
            self._render_current_table()

    def _on_main_price_sync_failed(self, message: str) -> None:
        self._render_current_table()
        self.status.showMessage(f"Price sync failed: {message[:80]}", 5000)

    def _local_row_update_from_search(self, category: str, ipn: str, updates: dict[str, str]) -> dict[str, str] | None:
        return self._apply_row_updates(category, ipn, updates)

    def _decorate_price_range_item(self, item: QTableWidgetItem, row: dict[str, str]) -> None:
        price_range = (row.get("Price_Range", "") or "").strip() or "?"
        item.setText(price_range)
        last_sync = (row.get("Price_LastSynced_UTC", "") or "").strip()
        digikey_price = (row.get("DigiKey_Price", "") or "").strip() or "Not Specified"
        mouser_price = (row.get("Mouser_Price", "") or "").strip() or "Not Specified"
        item.setToolTip(
            "\n".join(
                [
                    f"DigiKey: {digikey_price}",
                    f"Mouser: {mouser_price}",
                    f"Last Synced: {last_sync or 'Never'}",
                ]
            )
        )
        if not last_sync:
            item.setBackground(QColor("#4a2f2f"))
            return
        try:
            parsed = datetime.fromisoformat(last_sync.replace("Z", "+00:00"))
            if parsed.tzinfo is None:
                parsed = parsed.replace(tzinfo=timezone.utc)
            stale = (datetime.now(timezone.utc) - parsed) > timedelta(days=7)
        except ValueError:
            stale = True
        item.setBackground(QColor("#4a4930") if stale else QColor(Qt.GlobalColor.transparent))

    @staticmethod
    def _reference_kind_for_header(header: str) -> str | None:
        if header == "Symbol":
            return "symbol"
        if header == "Footprint":
            return "footprint"
        return None

    def _reference_missing_local_reason(self, header: str, value: str) -> str | None:
        kind = self._reference_kind_for_header(header)
        ref = value.strip()
        if kind is None or not ref:
            return None
        if ":" not in ref:
            return "Invalid reference format (expected Library:Name)."
        entry = self.library_index.resolve(ref, kind)
        if entry is None:
            return "Reference not found in indexed libraries."
        if entry.source != "local":
            return "Reference exists only in KiCad libraries (not local)."
        return None

    def _decorate_library_reference_item(self, item: QTableWidgetItem, header: str, value: str) -> None:
        reason = self._reference_missing_local_reason(header, value)
        if not reason:
            return
        item.setBackground(QColor("#4a2f2f"))
        existing_tooltip = item.toolTip().strip()
        if existing_tooltip:
            item.setToolTip(f"{existing_tooltip}\n{reason}")
        else:
            item.setToolTip(reason)

    def _kicad_reference_candidates(self, kind: str, query: str, limit: int = 300) -> list[str]:
        seen: set[str] = set()
        candidates: list[str] = []

        def add(entries) -> bool:
            for entry in entries:
                if entry.source != "kicad":
                    continue
                key = entry.name.lower()
                if key in seen:
                    continue
                seen.add(key)
                candidates.append(entry.name)
                if len(candidates) >= limit:
                    return True
            return False

        search_text = query.strip()
        if search_text:
            if add(self.library_index.search(search_text, kind, limit=limit)):
                return candidates
            short_text = search_text.split(":", 1)[-1] if ":" in search_text else search_text
            if short_text != search_text and add(self.library_index.search(short_text, kind, limit=limit)):
                return candidates
            tokens = [tok for tok in short_text.replace(":", " ").replace("_", " ").replace("-", " ").split() if tok]
            if add(self.library_index.fuzzy_match(tokens, kind, limit=limit)):
                return candidates
        else:
            add(self.library_index.entries(kind))
        return candidates

    def _set_cell_value(self, row: int, col: int, new_value: str) -> None:
        if not self.current_category:
            return
        state = self.categories[self.current_category]
        model = state.model
        if row < 0 or row >= len(model.rows) or col < 0 or col >= len(model.headers):
            return
        header = model.headers[col]
        old_value = model.rows[row].get(header, "")
        if old_value == new_value:
            return
        self.undo_stack.push(SetCellCommand(model, row, col, old_value, new_value))
        self._render_current_table()
        self.table.setCurrentCell(row, col)

    def _search_in_kicad_libs_for_cell(self, row: int, col: int) -> None:
        if not self.current_category:
            return
        headers = self.categories[self.current_category].model.headers
        if col < 0 or col >= len(headers):
            return
        header = headers[col]
        kind = self._reference_kind_for_header(header)
        if kind is None:
            return
        item = self.table.item(row, col)
        current_value = item.text().strip() if item else ""
        candidates = self._kicad_reference_candidates(kind, current_value)
        if not candidates:
            QMessageBox.information(
                self,
                "Search in KiCad libs",
                f"No KiCad {header.lower()} matches found for '{current_value}'.",
            )
            return
        preferred_index = 0
        if current_value:
            wanted = current_value.lower()
            for idx, candidate in enumerate(candidates):
                if candidate.lower() == wanted:
                    preferred_index = idx
                    break
        picked, ok = QInputDialog.getItem(
            self,
            "Search in KiCad libs",
            f"{header} reference",
            candidates,
            preferred_index,
            False,
        )
        if not ok or not picked:
            return
        resolved = self._maybe_copy_external_reference(header, picked)
        self._set_cell_value(row, col, resolved)

    def _open_search_dialog(self, query: str, *, force_remote: bool = False, exact_mpn: bool = False) -> None:
        query_text = query.strip()
        dialog = SupplierSearchDialog(
            self.supplier_api,
            self,
            local_search=self._local_search_summary,
            local_row_update=self._local_row_update_from_search,
        )
        if query_text:
            dialog.query.setText(query_text)
            dialog.run_search(force_remote=force_remote, exact_mpn=exact_mpn)
        else:
            dialog.focus_query()
        dialog.exec()

    def _row_text_by_header(self, row: int, header: str) -> str:
        if not self.current_category:
            return ""
        headers = self.categories[self.current_category].model.headers
        if header not in headers:
            return ""
        col = headers.index(header)
        item = self.table.item(row, col)
        return item.text().strip() if item else ""

    @staticmethod
    def _looks_like_url(value: str) -> bool:
        return value.startswith("http://") or value.startswith("https://")

    @staticmethod
    def _has_supplier_pn_value(value: str) -> bool:
        text = value.strip().lower()
        return bool(text) and text not in {"n/a", "na", "not specified", "not found", "none", "null", "-", "--"}

    def _supplier_url_for_row(self, row: int, supplier: str) -> str:
        if supplier == "digikey":
            value = self._row_text_by_header(row, "DigiKey_PN")
            if self._looks_like_url(value):
                return value
            pn = value.strip()
            if not self._has_supplier_pn_value(pn):
                return ""
            return f"https://www.digikey.com/en/products/result?keywords={quote(pn)}" if pn else ""
        if supplier == "mouser":
            value = self._row_text_by_header(row, "Mouser_PN")
            if self._looks_like_url(value):
                return value
            pn = value.strip()
            if not self._has_supplier_pn_value(pn):
                return ""
            return f"https://www.mouser.com/c/?q={quote(pn)}" if pn else ""
        return ""

    def _open_main_table_context_menu(self, pos) -> None:
        if not self.current_category:
            return
        item = self.table.itemAt(pos)
        if item is None:
            return
        row = item.row()
        selected_rows = self._selected_rows()
        if row not in selected_rows:
            self.table.setCurrentCell(row, item.column())
            selected_rows = [row]

        if len(selected_rows) > 1:
            self._open_multi_row_context_menu(pos, selected_rows)
        else:
            self._open_single_row_context_menu(pos, row, item.column())

    def _open_single_row_context_menu(self, pos, row: int, clicked_col: int) -> None:
        headers = self.categories[self.current_category].model.headers
        clicked_header = headers[clicked_col] if 0 <= clicked_col < len(headers) else ""
        clicked_item = self.table.item(row, clicked_col)
        clicked_value = clicked_item.text().strip() if clicked_item else ""
        library_reference_reason = self._reference_missing_local_reason(clicked_header, clicked_value)
        datasheet_url = self._row_text_by_header(row, "Datasheet")
        digikey_url = self._supplier_url_for_row(row, "digikey")
        mouser_url = self._supplier_url_for_row(row, "mouser")
        description = self._row_text_by_header(row, "Description")
        value = self._row_text_by_header(row, "Value")
        mpn = self._row_text_by_header(row, "MPN")
        similar_query = " ".join(part for part in (description, value) if part).strip()

        menu = QMenu(self)
        open_datasheet_action = QAction("Open datasheet", self)
        open_digikey_action = QAction("Open in DigiKey", self)
        open_mouser_action = QAction("Open in Mouser", self)
        search_similar_action = QAction("Search similar specs", self)
        search_same_mpn_action = QAction("Search same MPN", self)
        assign_supplier_action = QAction("Assign DigiKey + Mouser PN", self)
        sync_prices_action = QAction("Sync Prices", self)
        search_kicad_libs_action = QAction("Search in KiCad libs", self)
        dk_pn = self._row_text_by_header(row, "DigiKey_PN").strip()
        mouser_pn = self._row_text_by_header(row, "Mouser_PN").strip()
        has_dk_pn = self._has_supplier_pn_value(dk_pn)
        has_mouser_pn = self._has_supplier_pn_value(mouser_pn)

        open_datasheet_action.setEnabled(self._looks_like_url(datasheet_url))
        open_digikey_action.setEnabled(has_dk_pn and self._looks_like_url(digikey_url))
        open_mouser_action.setEnabled(has_mouser_pn and self._looks_like_url(mouser_url))
        search_similar_action.setEnabled(bool(similar_query))
        search_same_mpn_action.setEnabled(bool(mpn))
        assign_supplier_action.setEnabled(bool(mpn))
        sync_prices_action.setEnabled(has_dk_pn or has_mouser_pn)
        search_kicad_libs_action.setEnabled(bool(library_reference_reason))

        open_datasheet_action.triggered.connect(lambda: QDesktopServices.openUrl(QUrl(datasheet_url)))
        open_digikey_action.triggered.connect(lambda: QDesktopServices.openUrl(QUrl(digikey_url)))
        open_mouser_action.triggered.connect(lambda: QDesktopServices.openUrl(QUrl(mouser_url)))
        search_similar_action.triggered.connect(lambda: self._open_search_dialog(similar_query, force_remote=False))
        search_same_mpn_action.triggered.connect(lambda: self._open_search_dialog(mpn, exact_mpn=True))
        search_kicad_libs_action.triggered.connect(
            lambda _checked=False, r=row, c=clicked_col: self._search_in_kicad_libs_for_cell(r, c),
        )
        assign_supplier_action.triggered.connect(
            lambda _checked=False, rows=[row]: self._assign_supplier_pns_for_rows(rows),
        )
        sync_prices_action.triggered.connect(
            lambda _checked=False, rows=[row]: self._sync_prices_for_rows(rows),
        )

        menu.addAction(open_datasheet_action)
        menu.addAction(open_digikey_action)
        menu.addAction(open_mouser_action)
        menu.addSeparator()
        menu.addAction(search_similar_action)
        menu.addAction(search_same_mpn_action)
        if library_reference_reason:
            menu.addAction(search_kicad_libs_action)
        menu.addSeparator()
        menu.addAction(assign_supplier_action)
        menu.addAction(sync_prices_action)
        menu.exec(self.table.viewport().mapToGlobal(pos))

    def _open_multi_row_context_menu(self, pos, selected_rows: list[int]) -> None:
        has_mpn = all(bool(self._row_text_by_header(r, "MPN")) for r in selected_rows)
        has_pn = any(
            self._row_text_by_header(r, "DigiKey_PN") or self._row_text_by_header(r, "Mouser_PN")
            for r in selected_rows
        )

        menu = QMenu(self)
        count = len(selected_rows)

        assign_action = QAction(f"Assign DigiKey + Mouser PN ({count} items)", self)
        assign_action.setEnabled(has_mpn)
        assign_action.triggered.connect(
            lambda _checked=False, rows=selected_rows.copy(): self._assign_supplier_pns_for_rows(rows),
        )

        sync_action = QAction(f"Sync Prices ({count} items)", self)
        sync_action.setEnabled(has_pn)
        sync_action.triggered.connect(
            lambda _checked=False, rows=selected_rows.copy(): self._sync_prices_for_rows(rows),
        )

        menu.addAction(assign_action)
        menu.addAction(sync_action)
        menu.exec(self.table.viewport().mapToGlobal(pos))

    def _assign_supplier_pns_for_rows(self, rows: list[int]) -> None:
        if not self.current_category or not rows:
            return
        assignments: list[tuple[str, str, str]] = []
        for row in rows:
            ipn = self._row_text_by_header(row, "IPN")
            mpn = self._row_text_by_header(row, "MPN")
            if not ipn or not mpn:
                continue
            assignments.append((self.current_category, ipn, mpn))
        if not assignments:
            QMessageBox.information(self, "Assign DigiKey + Mouser PN", "No valid rows selected for assignment.")
            return
        dialog = PnAssignProgressDialog(
            self.supplier_api,
            assignments,
            on_row_update=self._apply_supplier_assignment_row,
            parent=self,
        )
        dialog.exec()
        if self.current_category:
            self._render_current_table()

    def _sync_prices_for_rows(self, rows: list[int]) -> None:
        if not self.current_category or not rows:
            return
        state = self.categories.get(self.current_category)
        if state is None:
            return
        assignments: list[tuple[str, str, str]] = []
        label_lookup: dict[tuple[str, str], str] = {}
        for row in rows:
            ipn = self._row_text_by_header(row, "IPN")
            if not ipn:
                continue
            row_idx = self._find_row_by_ipn(self.current_category, ipn)
            if row_idx < 0:
                continue
            row_data = state.model.rows[row_idx]
            dk_pn = row_data.get("DigiKey_PN", "").strip()
            mouser_pn = row_data.get("Mouser_PN", "").strip()
            if not dk_pn and not mouser_pn:
                continue
            mpn_hint = row_data.get("MPN", "").strip()
            packed = f"{dk_pn}\t{mouser_pn}\t{mpn_hint}"
            assignments.append((self.current_category, ipn, packed))
            label_lookup[(self.current_category, ipn)] = mpn_hint or ipn
        if not assignments:
            QMessageBox.information(self, "Sync Prices", "No rows with supplier PNs to sync.")
            return
        dialog = PriceSyncProgressDialog(
            self.supplier_api,
            assignments,
            label_lookup,
            on_row_update=self._apply_supplier_assignment_row,
            parent=self,
        )
        dialog.exec()
        if self.current_category:
            self._render_current_table()

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

    def save_all(self) -> None:
        saved_any = False
        for state in self.categories.values():
            if not state.dirty:
                continue
            state.document.rows = state.model.rows
            write_csv(state.document, make_backup=True)
            state.model.set_dirty(False)
            saved_any = True
        if saved_any and self.current_category:
            self._render_current_table()

    def search_all(self) -> None:
        ipn = self._selected_ipn()
        self._open_search_dialog(ipn, force_remote=False)

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
            self.save_all()
        event.accept()

