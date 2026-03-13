from __future__ import annotations

from datetime import datetime, timedelta, timezone
import traceback
import webbrowser
from collections.abc import Callable
from urllib.parse import quote

from PyQt6.QtCore import QEasingCurve, QPropertyAnimation, QThread, QTimer, Qt, pyqtSignal
from PyQt6.QtGui import QAction, QColor
from PyQt6.QtWidgets import (
    QAbstractItemView,
    QApplication,
    QDialog,
    QGraphicsOpacityEffect,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QLineEdit,
    QMenu,
    QMessageBox,
    QProgressBar,
    QPushButton,
    QTextEdit,
    QTreeWidget,
    QTreeWidgetItem,
    QVBoxLayout,
    QWidget,
)

from .error_handler import show_error_dialog
from .search import LocalSearchSummary, should_search_remote
from .supplier_api import SupplierApiClient, SupplierPart

ROLE_DATASHEET = Qt.ItemDataRole.UserRole + 1
ROLE_LOCAL = Qt.ItemDataRole.UserRole + 2
ROLE_DESCRIPTION = Qt.ItemDataRole.UserRole + 3
ROLE_VALUE = Qt.ItemDataRole.UserRole + 4
ROLE_RAW_SOURCE = Qt.ItemDataRole.UserRole + 5
ROLE_LOCAL_CATEGORY = Qt.ItemDataRole.UserRole + 6
ROLE_LOCAL_IPN = Qt.ItemDataRole.UserRole + 7
ROLE_LAST_SYNC = Qt.ItemDataRole.UserRole + 8
ROLE_DK_PRICE = Qt.ItemDataRole.UserRole + 9
ROLE_MOUSER_PRICE = Qt.ItemDataRole.UserRole + 10
ROLE_ROW_SNAPSHOT = Qt.ItemDataRole.UserRole + 11

EDITABLE_LOCAL_COLUMNS = {1: "MPN", 2: "Manufacturer", 3: "Description", 4: "DigiKey_PN", 5: "Mouser_PN"}


class SearchWorker(QThread):
    finished_results = pyqtSignal(list)
    failed = pyqtSignal(str, str)

    def __init__(
        self,
        client: SupplierApiClient,
        query: str,
        limit: int = 20,
        search_provider: Callable[[str, int], list[SupplierPart]] | None = None,
    ):
        super().__init__()
        self.client = client
        self.query = query
        self.limit = limit
        self.search_provider = search_provider or self.client.search_all

    def run(self) -> None:
        try:
            results = self.search_provider(self.query, self.limit)
        except Exception as exc:  # pragma: no cover
            self.failed.emit(str(exc), traceback.format_exc())
            return
        self.finished_results.emit(results)


class PriceSyncWorker(QThread):
    finished_prices = pyqtSignal(dict)
    failed = pyqtSignal(str)

    def __init__(
        self,
        client: SupplierApiClient,
        digikey_pn: str,
        mouser_pn: str,
        mpn_hint: str,
        category: str,
        ipn: str,
    ):
        super().__init__()
        self.client = client
        self.digikey_pn = digikey_pn
        self.mouser_pn = mouser_pn
        self.mpn_hint = mpn_hint
        self.category = category
        self.ipn = ipn

    def run(self) -> None:
        try:
            prices = self.client.fetch_supplier_prices(
                self.digikey_pn, self.mouser_pn, mpn_hint=self.mpn_hint,
            )
            prices["_category"] = self.category
            prices["_ipn"] = self.ipn
            prices["_timestamp"] = datetime.now(timezone.utc).isoformat()
            self.finished_prices.emit(prices)
        except Exception as exc:
            self.failed.emit(str(exc))


class _BatchWorkerBase(QThread):
    """Shared signal interface for batch operations (PN assignment, price sync)."""

    progress = pyqtSignal(int, int, str)
    row_done = pyqtSignal(str, str, dict)
    all_done = pyqtSignal()

    def __init__(self, client: SupplierApiClient, assignments: list[tuple[str, str, str]]):
        super().__init__()
        self.client = client
        self.assignments = assignments


class PnAssignWorker(_BatchWorkerBase):
    def run(self) -> None:
        total = len(self.assignments)
        for index, (category, ipn, mpn) in enumerate(self.assignments, start=1):
            self.progress.emit(index, total, mpn)
            try:
                updates = self.client.resolve_supplier_pns(mpn)
                updates["Price_LastSynced_UTC"] = datetime.now(timezone.utc).isoformat()
            except Exception as exc:  # pragma: no cover
                updates = {"_error": str(exc)}
            self.row_done.emit(category, ipn, updates)
        self.all_done.emit()


class PriceSyncBatchWorker(_BatchWorkerBase):
    """Fetches prices for rows that already have DigiKey/Mouser PNs assigned.

    ``assignments`` items are ``(category, ipn, dk_pn|mouser_pn|mpn_hint)``
    packed as ``(category, ipn, "dk_pn\\tmouser_pn\\tmpn_hint")``.
    """

    def run(self) -> None:
        total = len(self.assignments)
        for index, (category, ipn, packed) in enumerate(self.assignments, start=1):
            parts = packed.split("\t")
            dk_pn = parts[0] if len(parts) > 0 else ""
            mouser_pn = parts[1] if len(parts) > 1 else ""
            mpn_hint = parts[2] if len(parts) > 2 else ""
            self.progress.emit(index, total, mpn_hint or dk_pn or mouser_pn)
            try:
                prices = self.client.fetch_supplier_prices(dk_pn, mouser_pn, mpn_hint=mpn_hint)
                updates: dict[str, str] = {
                    "DigiKey_Price": prices.get("DigiKey_Price", ""),
                    "Mouser_Price": prices.get("Mouser_Price", ""),
                    "Price_Range": prices.get("Price_Range", "?") or "?",
                    "Price_LastSynced_UTC": datetime.now(timezone.utc).isoformat(),
                }
            except Exception as exc:  # pragma: no cover
                updates = {"_error": str(exc)}
            self.row_done.emit(category, ipn, updates)
        self.all_done.emit()


class BatchProgressDialog(QDialog):
    """Non-cancellable progress dialog for batch operations."""

    def __init__(
        self,
        title: str,
        worker: _BatchWorkerBase,
        label_lookup: dict[tuple[str, str], str],
        format_row: Callable[[str, dict], str],
        on_row_update: Callable[[str, str, dict[str, str]], bool] | None = None,
        parent=None,
    ):
        super().__init__(parent)
        self.setWindowTitle(title)
        self.setModal(True)
        self.setWindowModality(Qt.WindowModality.ApplicationModal)
        self.setWindowFlag(Qt.WindowType.WindowCloseButtonHint, False)
        self.resize(660, 360)
        self._on_row_update = on_row_update
        self._format_row = format_row
        self._label_lookup = label_lookup
        self._completed = 0
        self._finished = False

        self.status_label = QLabel("Preparing...")
        self.progress_bar = QProgressBar()
        self.progress_bar.setRange(0, max(1, len(label_lookup)))
        self.progress_bar.setValue(0)
        self.log = QTextEdit()
        self.log.setReadOnly(True)
        self.done_btn = QPushButton("Done")
        self.done_btn.setEnabled(False)
        self.done_btn.clicked.connect(self.accept)

        layout = QVBoxLayout()
        layout.addWidget(self.status_label)
        layout.addWidget(self.progress_bar)
        layout.addWidget(self.log, 1)
        layout.addWidget(self.done_btn, 0, Qt.AlignmentFlag.AlignRight)
        self.setLayout(layout)

        self._worker = worker
        self._worker.progress.connect(self._on_progress)
        self._worker.row_done.connect(self._on_row_done)
        self._worker.all_done.connect(self._on_all_done)
        self._worker.start()

    def reject(self) -> None:
        if self._finished:
            super().reject()

    def keyPressEvent(self, event) -> None:  # type: ignore[override]
        if not self._finished and event.key() == Qt.Key.Key_Escape:
            event.ignore()
            return
        super().keyPressEvent(event)

    def _on_progress(self, current: int, total: int, label: str) -> None:
        self.status_label.setText(f"Processing {current} of {total}: {label}")

    def _on_row_done(self, category: str, ipn: str, updates: dict) -> None:
        self._completed += 1
        self.progress_bar.setValue(self._completed)
        label = self._label_lookup.get((category, ipn), ipn)
        if "_error" in updates:
            self.log.append(f"[ERROR] {label}: {updates.get('_error', 'Unknown error')}")
            return
        applied = self._on_row_update(category, ipn, updates) if self._on_row_update else True
        status = "Updated" if applied else "Skipped"
        detail = self._format_row(label, updates)
        self.log.append(f"[{status}] {detail}")

    def _on_all_done(self) -> None:
        self._finished = True
        self.status_label.setText(f"Completed {self._completed} item(s).")
        self.done_btn.setEnabled(True)


def _format_assign_row(label: str, updates: dict) -> str:
    dk = updates.get("DigiKey_PN", "").strip() or "Not Found"
    mouser = updates.get("Mouser_PN", "").strip() or "Not Found"
    price = updates.get("Price_Range", "?") or "?"
    return f"{label} -> DK: {dk}, Mouser: {mouser}, Price: {price}"


def _format_price_sync_row(label: str, updates: dict) -> str:
    price = updates.get("Price_Range", "?") or "?"
    return f"{label} -> Price: {price}"


class PnAssignProgressDialog(BatchProgressDialog):
    def __init__(
        self,
        client: SupplierApiClient,
        assignments: list[tuple[str, str, str]],
        on_row_update: Callable[[str, str, dict[str, str]], bool] | None = None,
        parent=None,
    ):
        label_lookup = {(cat, ipn): mpn for cat, ipn, mpn in assignments}
        worker = PnAssignWorker(client, assignments)
        super().__init__(
            "Assign DigiKey + Mouser PN",
            worker,
            label_lookup,
            _format_assign_row,
            on_row_update=on_row_update,
            parent=parent,
        )


class PriceSyncProgressDialog(BatchProgressDialog):
    def __init__(
        self,
        client: SupplierApiClient,
        assignments: list[tuple[str, str, str]],
        label_lookup: dict[tuple[str, str], str],
        on_row_update: Callable[[str, str, dict[str, str]], bool] | None = None,
        parent=None,
    ):
        worker = PriceSyncBatchWorker(client, assignments)
        super().__init__(
            "Sync Prices",
            worker,
            label_lookup,
            _format_price_sync_row,
            on_row_update=on_row_update,
            parent=parent,
        )


class ToastNotification(QLabel):
    def __init__(self, parent: QWidget, text: str, duration_ms: int = 1800):
        super().__init__(text, parent)
        self.setStyleSheet(
            "background-color: rgba(40, 40, 40, 220);"
            "color: #e0e0e0;"
            "padding: 8px 18px;"
            "border-radius: 8px;"
            "font-size: 13px;"
        )
        self.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.adjustSize()
        pw, ph = parent.width(), parent.height()
        self.move((pw - self.width()) // 2, ph - self.height() - 48)
        self._opacity = QGraphicsOpacityEffect(self)
        self._opacity.setOpacity(1.0)
        self.setGraphicsEffect(self._opacity)
        self.show()
        self.raise_()
        QTimer.singleShot(duration_ms, self._fade_out)

    def _fade_out(self) -> None:
        self._anim = QPropertyAnimation(self._opacity, b"opacity")
        self._anim.setDuration(400)
        self._anim.setStartValue(1.0)
        self._anim.setEndValue(0.0)
        self._anim.setEasingCurve(QEasingCurve.Type.OutCubic)
        self._anim.finished.connect(self.deleteLater)
        self._anim.start()


class SupplierSearchDialog(QDialog):
    def __init__(
        self,
        client: SupplierApiClient,
        parent=None,
        local_search: Callable[[str], LocalSearchSummary] | None = None,
        local_row_update: Callable[[str, str, dict[str, str]], dict[str, str] | None] | None = None,
    ):
        super().__init__(parent)
        self.client = client
        self.local_search = local_search
        self.local_row_update = local_row_update
        self.selected_part: SupplierPart | None = None
        self._worker: SearchWorker | None = None
        self._price_worker: PriceSyncWorker | None = None
        self._local_result_count = 0
        self._suppress_item_changed = False
        self.setWindowTitle("Supplier Search")
        self.resize(1000, 420)

        self.query = QLineEdit()
        self.query.setPlaceholderText("Search by keyword or part number")
        self.search_btn = QPushButton("Search")
        self.search_btn.clicked.connect(self.run_search)
        self.force_remote_btn = QPushButton("Search DigiKey + Mouser")
        self.force_remote_btn.clicked.connect(lambda: self.run_search(force_remote=True))
        self.force_remote_btn.setVisible(self.local_search is not None)
        self.force_remote_btn.setEnabled(self.local_search is not None)
        self.status_note = QLabel("")

        top = QHBoxLayout()
        top.addWidget(QLabel("Query:"))
        top.addWidget(self.query, 1)
        top.addWidget(self.search_btn)
        top.addWidget(self.force_remote_btn)

        self.spinner = QProgressBar()
        self.spinner.setRange(0, 0)
        self.spinner.setTextVisible(False)
        self.spinner.hide()

        self.results = QTreeWidget()
        self.results.setColumnCount(8)
        self.results.setEditTriggers(
            QAbstractItemView.EditTrigger.DoubleClicked | QAbstractItemView.EditTrigger.EditKeyPressed
        )
        self.results.setHeaderLabels(
            [
                "Source",
                "MPN",
                "Manufacturer",
                "Description",
                "DigiKey PN",
                "Mouser PN",
                "Qty Available",
                "Price",
            ]
        )
        header = self.results.header()
        header.setSectionResizeMode(QHeaderView.ResizeMode.Interactive)
        header.setStretchLastSection(True)
        self.results.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self.results.customContextMenuRequested.connect(self._open_result_context_menu)
        self.results.itemChanged.connect(self._on_result_item_changed)
        self.results.itemDoubleClicked.connect(self._on_item_double_clicked)

        button_row = QHBoxLayout()
        pick_btn = QPushButton("Use Selected")
        pick_btn.clicked.connect(self.accept_selected)
        cancel_btn = QPushButton("Cancel")
        cancel_btn.clicked.connect(self.reject)
        button_row.addStretch(1)
        button_row.addWidget(pick_btn)
        button_row.addWidget(cancel_btn)

        layout = QVBoxLayout()
        layout.addLayout(top)
        layout.addWidget(self.status_note)
        layout.addWidget(self.spinner)
        layout.addWidget(self.results, 1)
        layout.addLayout(button_row)
        self.setLayout(layout)

    def focus_query(self) -> None:
        self.query.setFocus()
        self.query.selectAll()

    def run_search(self, force_remote: bool = False, *, exact_mpn: bool = False) -> None:
        query = self.query.text().strip()
        if not query:
            return
        if self._worker and self._worker.isRunning():
            return
        self.results.clear()
        self._local_result_count = 0
        self.selected_part = None
        local_summary: LocalSearchSummary | None = None
        if self.local_search is not None:
            local_summary = self.local_search(query)
            if exact_mpn and local_summary:
                exact = [h for h in local_summary.hits if h.mpn.strip().lower() == query.lower()]
                local_summary = LocalSearchSummary(
                    query=local_summary.query,
                    hits=exact,
                    confidence="high" if exact else "none",
                    best_score=exact[0].score if exact else 0.0,
                )
            self._render_local_results(local_summary)

        should_fetch_remote = exact_mpn or self.local_search is None or should_search_remote(
            local_summary or LocalSearchSummary(query=query, hits=[], confidence="none", best_score=0.0),
            force_remote=force_remote,
        )
        if self.local_search is not None:
            if should_fetch_remote:
                self.status_note.setText("Searching DigiKey and Mouser...")
            else:
                level = local_summary.confidence.upper() if local_summary else "NONE"
                self.status_note.setText(
                    f"Local confidence: {level}. Use Search DigiKey + Mouser if this is not the right part."
                )
        else:
            self.status_note.setText("Searching DigiKey and Mouser...")

        if not should_fetch_remote:
            return

        self.search_btn.setEnabled(False)
        self.force_remote_btn.setEnabled(False)
        self.spinner.show()
        search_provider = self.client.search_by_mpn if exact_mpn else None
        self._worker = SearchWorker(self.client, query, limit=20, search_provider=search_provider)
        self._worker.finished_results.connect(self._on_search_results)
        self._worker.failed.connect(self._on_search_failed)
        self._worker.finished.connect(self._on_search_done)
        self._worker.start()

    def _on_search_done(self) -> None:
        self.search_btn.setEnabled(True)
        self.force_remote_btn.setEnabled(self.local_search is not None)
        self.spinner.hide()

    def _on_search_failed(self, message: str, details: str) -> None:
        show_error_dialog(self, "Supplier search failed", message, details)

    def _on_search_results(self, results: list[SupplierPart]) -> None:
        digikey = [part for part in results if part.source.lower().startswith("digikey")]
        mouser = [part for part in results if part.source.lower().startswith("mouser")]

        digikey.sort(key=lambda part: part.quantity_available, reverse=True)
        mouser.sort(key=lambda part: part.quantity_available, reverse=True)

        for title, parts in (("DigiKey", digikey), ("Mouser", mouser)):
            group = QTreeWidgetItem([f"{title} ({len(parts)})"])
            group.setFirstColumnSpanned(True)
            group.setFlags(group.flags() & ~Qt.ItemFlag.ItemIsSelectable)
            for part in parts:
                values = [
                    part.source,
                    part.mpn,
                    part.manufacturer,
                    part.description,
                    part.digikey_pn,
                    part.mouser_pn,
                    str(part.quantity_available),
                    part.price,
                ]
                child = QTreeWidgetItem(values)
                child.setData(0, ROLE_DATASHEET, part.datasheet)
                child.setData(0, ROLE_DESCRIPTION, part.description)
                child.setData(0, ROLE_VALUE, "")
                child.setData(0, ROLE_RAW_SOURCE, part.source)
                if part.product_url:
                    link_col = 4 if part.source.lower().startswith("digikey") else 5
                    child.setData(link_col, Qt.ItemDataRole.UserRole, part.product_url)
                    child.setToolTip(link_col, part.product_url)
                group.addChild(child)
            self.results.addTopLevelItem(group)
            group.setExpanded(bool(parts))
        if not results and self._local_result_count == 0:
            self.status_note.setText("No local or supplier matches found.")
        elif not results and self._local_result_count > 0:
            self.status_note.setText("No supplier matches found. Showing local matches only.")
        elif results:
            self.status_note.setText("Showing local and supplier matches.")

    def _render_local_results(self, summary: LocalSearchSummary) -> None:
        self._local_result_count = len(summary.hits)
        group = QTreeWidgetItem([f"My Parts ({len(summary.hits)})"])
        group.setFirstColumnSpanned(True)
        group.setFlags(group.flags() & ~Qt.ItemFlag.ItemIsSelectable)
        self._suppress_item_changed = True
        for hit in summary.hits:
            price_range = hit.price_range.strip() or "?"
            values = [
                "My Parts",
                hit.mpn,
                hit.manufacturer,
                hit.description,
                hit.digikey_pn.strip() or "Not Specified",
                hit.mouser_pn.strip() or "Not Specified",
                "Not Specified",
                price_range,
            ]
            child = QTreeWidgetItem(values)
            child.setFlags(child.flags() | Qt.ItemFlag.ItemIsEditable)
            child.setData(0, ROLE_LOCAL, "local")
            child.setData(0, ROLE_DATASHEET, hit.datasheet)
            child.setData(0, ROLE_DESCRIPTION, hit.description)
            child.setData(0, ROLE_VALUE, hit.value or "")
            child.setData(0, ROLE_LOCAL_CATEGORY, hit.category)
            child.setData(0, ROLE_LOCAL_IPN, hit.ipn)
            child.setData(0, ROLE_LAST_SYNC, hit.price_last_synced_utc)
            child.setData(0, ROLE_DK_PRICE, hit.digikey_price)
            child.setData(0, ROLE_MOUSER_PRICE, hit.mouser_price)
            child.setToolTip(
                3,
                f"IPN: {hit.ipn}\nCategory: {hit.category.upper()}\nValue: {hit.value or 'N/A'}\nMatch Score: {hit.score:.2f}",
            )
            self._apply_price_cell_style(child)
            child.setData(0, ROLE_ROW_SNAPSHOT, [child.text(col) for col in range(self.results.columnCount())])
            group.addChild(child)
        self._suppress_item_changed = False
        self.results.addTopLevelItem(group)
        group.setExpanded(bool(summary.hits))

    def _apply_price_cell_style(self, item: QTreeWidgetItem) -> None:
        dk_price = item.data(0, ROLE_DK_PRICE) or ""
        mouser_price = item.data(0, ROLE_MOUSER_PRICE) or ""
        last_sync = item.data(0, ROLE_LAST_SYNC) or ""
        last_sync_text = str(last_sync).strip()
        if not last_sync_text:
            item.setBackground(7, QColor("#4a2f2f"))
        else:
            try:
                parsed = datetime.fromisoformat(last_sync_text.replace("Z", "+00:00"))
                if parsed.tzinfo is None:
                    parsed = parsed.replace(tzinfo=timezone.utc)
                is_stale = (datetime.now(timezone.utc) - parsed) > timedelta(days=7)
            except ValueError:
                is_stale = True
            item.setBackground(7, QColor("#4a4930") if is_stale else QColor(Qt.GlobalColor.transparent))
        item.setToolTip(
            7,
            "\n".join(
                [
                    f"DigiKey: {str(dk_price).strip() or 'Not Specified'}",
                    f"Mouser: {str(mouser_price).strip() or 'Not Specified'}",
                    f"Last Synced: {last_sync_text or 'Never'}",
                ]
            ),
        )

    def _item_datasheet_url(self, item: QTreeWidgetItem) -> str:
        value = item.data(0, ROLE_DATASHEET)
        return value if isinstance(value, str) else ""

    def _item_supplier_url(self, item: QTreeWidgetItem, supplier: str) -> str:
        link_col = 4 if supplier == "digikey" else 5
        url = item.data(link_col, Qt.ItemDataRole.UserRole)
        if isinstance(url, str) and url.startswith(("http://", "https://")):
            return url
        pn = item.text(link_col).strip()
        if not pn or pn.lower() == "not specified":
            return ""
        if supplier == "digikey":
            return f"https://www.digikey.com/en/products/result?keywords={quote(pn)}"
        if supplier == "mouser":
            return f"https://www.mouser.com/c/?q={quote(pn)}"
        return ""

    @staticmethod
    def _has_supplier_pn(item: QTreeWidgetItem, supplier: str) -> bool:
        col = 4 if supplier == "digikey" else 5
        pn = item.text(col).strip().lower()
        return bool(pn) and pn not in {"not specified", "not found", "n/a", "na", "none", "null", "-", "--"}

    def _open_url_in_browser(self, url: str) -> None:
        if not url.startswith(("http://", "https://")):
            return
        webbrowser.open(url, new=2)

    def _similar_query_for_item(self, item: QTreeWidgetItem) -> str:
        description = item.data(0, ROLE_DESCRIPTION)
        value = item.data(0, ROLE_VALUE)
        desc_text = description.strip() if isinstance(description, str) else item.text(3).strip()
        value_text = value.strip() if isinstance(value, str) else ""
        if desc_text and value_text:
            return f"{desc_text} {value_text}"
        return desc_text

    def _open_result_context_menu(self, pos) -> None:
        item = self.results.itemAt(pos)
        if item is None or item.parent() is None:
            return
        self.results.setCurrentItem(item)

        datasheet_url = self._item_datasheet_url(item)
        digikey_url = self._item_supplier_url(item, "digikey")
        mouser_url = self._item_supplier_url(item, "mouser")

        menu = QMenu(self)
        use_selected_action = QAction("Use selected", self)
        open_datasheet_action = QAction("Open datasheet", self)
        open_digikey_action = QAction("Open in DigiKey", self)
        open_mouser_action = QAction("Open in Mouser", self)
        search_similar_action = QAction("Search similar specs", self)
        search_same_mpn_action = QAction("Search same MPN", self)

        use_selected_action.triggered.connect(self.accept_selected)
        open_datasheet_action.triggered.connect(lambda: self._open_url_in_browser(datasheet_url))
        open_digikey_action.triggered.connect(lambda: self._open_url_in_browser(digikey_url))
        open_mouser_action.triggered.connect(lambda: self._open_url_in_browser(mouser_url))
        search_similar_action.triggered.connect(lambda: self._search_similar_specs(item))
        search_same_mpn_action.triggered.connect(lambda: self._search_same_mpn(item))

        is_local = item.data(0, ROLE_LOCAL) == "local"
        if is_local:
            use_selected_action.setEnabled(False)
        open_datasheet_action.setEnabled(datasheet_url.startswith(("http://", "https://")))
        open_digikey_action.setEnabled(self._has_supplier_pn(item, "digikey") and digikey_url.startswith(("http://", "https://")))
        open_mouser_action.setEnabled(self._has_supplier_pn(item, "mouser") and mouser_url.startswith(("http://", "https://")))

        menu.addAction(use_selected_action)
        menu.addAction(open_datasheet_action)
        menu.addAction(open_digikey_action)
        menu.addAction(open_mouser_action)
        menu.addSeparator()
        menu.addAction(search_similar_action)
        menu.addAction(search_same_mpn_action)
        menu.exec(self.results.viewport().mapToGlobal(pos))

    def _search_similar_specs(self, item: QTreeWidgetItem) -> None:
        query = self._similar_query_for_item(item)
        if not query:
            QMessageBox.information(self, "Search similar specs", "No searchable details found for this row.")
            return
        self.query.setText(query)
        self.run_search(force_remote=False)

    def _search_same_mpn(self, item: QTreeWidgetItem) -> None:
        mpn = item.text(1).strip()
        if not mpn:
            QMessageBox.information(self, "Search same MPN", "No MPN is available for this row.")
            return
        self.query.setText(mpn)
        self.run_search(exact_mpn=True)

    def _on_result_item_changed(self, item: QTreeWidgetItem, _column: int) -> None:
        if self._suppress_item_changed:
            return
        if item.parent() is None:
            return
        if item.data(0, ROLE_LOCAL) != "local":
            return
        category = item.data(0, ROLE_LOCAL_CATEGORY)
        ipn = item.data(0, ROLE_LOCAL_IPN)
        if not isinstance(category, str) or not isinstance(ipn, str) or not category or not ipn:
            return
        snapshot = item.data(0, ROLE_ROW_SNAPSHOT)
        previous = list(snapshot) if isinstance(snapshot, list) else [item.text(col) for col in range(self.results.columnCount())]
        updates: dict[str, str] = {}
        reverted = False
        self._suppress_item_changed = True
        try:
            for col_idx, header in EDITABLE_LOCAL_COLUMNS.items():
                current_value = item.text(col_idx)
                prior_value = previous[col_idx] if col_idx < len(previous) else ""
                if current_value == prior_value:
                    continue
                updates[header] = current_value.strip()
            for col_idx in range(self.results.columnCount()):
                if col_idx in EDITABLE_LOCAL_COLUMNS:
                    continue
                prior_value = previous[col_idx] if col_idx < len(previous) else item.text(col_idx)
                if item.text(col_idx) != prior_value:
                    item.setText(col_idx, prior_value)
                    reverted = True
        finally:
            self._suppress_item_changed = False
        if updates and self.local_row_update:
            updated_row = self.local_row_update(category, ipn, updates)
            if updated_row:
                self._refresh_local_item_from_row(item, updated_row)
            else:
                reverted = True
        if reverted and not updates:
            return
        item.setData(0, ROLE_ROW_SNAPSHOT, [item.text(col) for col in range(self.results.columnCount())])

    def _refresh_local_item_from_row(self, item: QTreeWidgetItem, row: dict[str, str]) -> None:
        self._suppress_item_changed = True
        try:
            item.setText(1, row.get("MPN", ""))
            item.setText(2, row.get("Manufacturer", ""))
            item.setText(3, row.get("Description", ""))
            item.setText(4, row.get("DigiKey_PN", "").strip() or "Not Specified")
            item.setText(5, row.get("Mouser_PN", "").strip() or "Not Specified")
            item.setText(7, row.get("Price_Range", "").strip() or "?")
            item.setData(0, ROLE_DATASHEET, row.get("Datasheet", ""))
            item.setData(0, ROLE_DESCRIPTION, row.get("Description", ""))
            item.setData(0, ROLE_VALUE, row.get("Value", ""))
            item.setData(0, ROLE_LAST_SYNC, row.get("Price_LastSynced_UTC", ""))
            item.setData(0, ROLE_DK_PRICE, row.get("DigiKey_Price", ""))
            item.setData(0, ROLE_MOUSER_PRICE, row.get("Mouser_Price", ""))
            self._apply_price_cell_style(item)
            item.setData(0, ROLE_ROW_SNAPSHOT, [item.text(col) for col in range(self.results.columnCount())])
        finally:
            self._suppress_item_changed = False

    def _on_item_double_clicked(self, item: QTreeWidgetItem, _column: int) -> None:
        if item.parent() is None:
            return
        if item.data(0, ROLE_LOCAL) == "local":
            if _column == 7:
                self._start_dialog_price_sync(item)
            return
        copied_text = item.text(_column)
        QApplication.clipboard().setText(copied_text)
        ToastNotification(self, f"Copied: {copied_text}" if copied_text else "Copied empty cell")

    def _has_supplier_pns(self, item: QTreeWidgetItem) -> bool:
        dk = item.text(4).strip()
        mo = item.text(5).strip()
        return bool(dk and dk != "Not Specified") or bool(mo and mo != "Not Specified")

    def _start_dialog_price_sync(self, item: QTreeWidgetItem) -> None:
        if not self._has_supplier_pns(item):
            return
        if self._price_worker and self._price_worker.isRunning():
            return
        category = item.data(0, ROLE_LOCAL_CATEGORY)
        ipn = item.data(0, ROLE_LOCAL_IPN)
        if not isinstance(category, str) or not isinstance(ipn, str) or not category or not ipn:
            return
        dk_pn = item.text(4).strip()
        mo_pn = item.text(5).strip()
        dk_pn = "" if dk_pn == "Not Specified" else dk_pn
        mo_pn = "" if mo_pn == "Not Specified" else mo_pn
        mpn_hint = item.text(1).strip()

        loading = QProgressBar()
        loading.setRange(0, 0)
        loading.setTextVisible(False)
        loading.setMaximumHeight(20)
        self.results.setItemWidget(item, 7, loading)

        self._price_worker = PriceSyncWorker(
            self.client, dk_pn, mo_pn, mpn_hint, category, ipn,
        )
        self._price_worker.finished_prices.connect(
            lambda prices, _item=item: self._on_dialog_price_sync_done(_item, prices),
        )
        self._price_worker.failed.connect(
            lambda msg, _item=item: self._on_dialog_price_sync_failed(_item, msg),
        )
        self._price_worker.start()

    def _on_dialog_price_sync_done(self, item: QTreeWidgetItem, prices: dict) -> None:
        self.results.removeItemWidget(item, 7)
        category = prices.pop("_category", "")
        ipn = prices.pop("_ipn", "")
        timestamp = prices.pop("_timestamp", "")
        updates = {
            "DigiKey_Price": prices.get("DigiKey_Price", ""),
            "Mouser_Price": prices.get("Mouser_Price", ""),
            "Price_Range": prices.get("Price_Range", "?") or "?",
            "Price_LastSynced_UTC": timestamp,
        }
        if self.local_row_update and category and ipn:
            updated_row = self.local_row_update(category, ipn, updates)
            if updated_row:
                self._refresh_local_item_from_row(item, updated_row)
                return
        self._suppress_item_changed = True
        try:
            item.setText(7, updates["Price_Range"])
            item.setData(0, ROLE_DK_PRICE, updates["DigiKey_Price"])
            item.setData(0, ROLE_MOUSER_PRICE, updates["Mouser_Price"])
            item.setData(0, ROLE_LAST_SYNC, updates["Price_LastSynced_UTC"])
            self._apply_price_cell_style(item)
        finally:
            self._suppress_item_changed = False

    def _on_dialog_price_sync_failed(self, item: QTreeWidgetItem, message: str) -> None:
        self.results.removeItemWidget(item, 7)
        ToastNotification(self, f"Price sync failed: {message[:60]}")

    def accept_selected(self) -> None:
        item = self.results.currentItem()
        if not item or item.parent() is None:
            return
        if item.data(0, ROLE_LOCAL) == "local":
            return
        source = item.data(0, ROLE_RAW_SOURCE)
        source_text = source if isinstance(source, str) and source else item.text(0)
        self.selected_part = SupplierPart(
            source=source_text,
            mpn=item.text(1),
            manufacturer=item.text(2),
            description=item.text(3),
            datasheet=item.data(0, ROLE_DATASHEET) or "",
            digikey_pn=item.text(4),
            mouser_pn=item.text(5),
            quantity_available=int(item.text(6)) if item.text(6).isdigit() else 0,
            price=item.text(7),
            product_url=item.data(4, Qt.ItemDataRole.UserRole) or item.data(5, Qt.ItemDataRole.UserRole) or "",
        )
        self.accept()

