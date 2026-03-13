from __future__ import annotations

import traceback

from PyQt6.QtCore import QThread, Qt, QUrl, pyqtSignal
from PyQt6.QtGui import QDesktopServices
from PyQt6.QtWidgets import (
    QDialog,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QLineEdit,
    QProgressBar,
    QPushButton,
    QTreeWidget,
    QTreeWidgetItem,
    QVBoxLayout,
)

from .error_handler import show_error_dialog
from .supplier_api import SupplierApiClient, SupplierPart


class SearchWorker(QThread):
    finished_results = pyqtSignal(list)
    failed = pyqtSignal(str, str)

    def __init__(self, client: SupplierApiClient, query: str, limit: int = 20):
        super().__init__()
        self.client = client
        self.query = query
        self.limit = limit

    def run(self) -> None:
        try:
            results = self.client.search_all(self.query, limit=self.limit)
        except Exception as exc:  # pragma: no cover
            self.failed.emit(str(exc), traceback.format_exc())
            return
        self.finished_results.emit(results)


class SupplierSearchDialog(QDialog):
    def __init__(self, client: SupplierApiClient, parent=None):
        super().__init__(parent)
        self.client = client
        self.selected_part: SupplierPart | None = None
        self._worker: SearchWorker | None = None
        self.setWindowTitle("Supplier Search")
        self.resize(1000, 420)

        self.query = QLineEdit()
        self.query.setPlaceholderText("Search by keyword or part number")
        self.search_btn = QPushButton("Search Both")
        self.search_btn.clicked.connect(self.run_search)

        top = QHBoxLayout()
        top.addWidget(QLabel("Query:"))
        top.addWidget(self.query, 1)
        top.addWidget(self.search_btn)

        self.spinner = QProgressBar()
        self.spinner.setRange(0, 0)
        self.spinner.setTextVisible(False)
        self.spinner.hide()

        self.results = QTreeWidget()
        self.results.setColumnCount(8)
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
        self.results.header().setSectionResizeMode(QHeaderView.ResizeMode.Stretch)
        self.results.itemDoubleClicked.connect(lambda *_: self.accept_selected())
        self.results.itemClicked.connect(self._on_item_clicked)

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
        layout.addWidget(self.spinner)
        layout.addWidget(self.results, 1)
        layout.addLayout(button_row)
        self.setLayout(layout)

    def run_search(self) -> None:
        query = self.query.text().strip()
        if not query:
            return
        if self._worker and self._worker.isRunning():
            return
        self.results.clear()
        self.search_btn.setEnabled(False)
        self.spinner.show()
        self._worker = SearchWorker(self.client, query, limit=20)
        self._worker.finished_results.connect(self._on_search_results)
        self._worker.failed.connect(self._on_search_failed)
        self._worker.finished.connect(self._on_search_done)
        self._worker.start()

    def _on_search_done(self) -> None:
        self.search_btn.setEnabled(True)
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
                child.setData(0, Qt.ItemDataRole.UserRole + 1, part.datasheet)
                if part.product_url:
                    link_col = 4 if part.source.lower().startswith("digikey") else 5
                    child.setData(link_col, Qt.ItemDataRole.UserRole, part.product_url)
                    child.setToolTip(link_col, part.product_url)
                    child.setForeground(link_col, Qt.GlobalColor.blue)
                group.addChild(child)
            self.results.addTopLevelItem(group)
            group.setExpanded(bool(parts))

    def _on_item_clicked(self, item: QTreeWidgetItem, column: int) -> None:
        if item.parent() is None:
            return
        url = item.data(column, Qt.ItemDataRole.UserRole)
        if isinstance(url, str) and url.startswith(("http://", "https://")):
            QDesktopServices.openUrl(QUrl(url))

    def accept_selected(self) -> None:
        item = self.results.currentItem()
        if not item or item.parent() is None:
            return
        self.selected_part = SupplierPart(
            source=item.text(0),
            mpn=item.text(1),
            manufacturer=item.text(2),
            description=item.text(3),
            datasheet=item.data(0, Qt.ItemDataRole.UserRole + 1) or "",
            digikey_pn=item.text(4),
            mouser_pn=item.text(5),
            quantity_available=int(item.text(6)) if item.text(6).isdigit() else 0,
            price=item.text(7),
            product_url=item.data(4, Qt.ItemDataRole.UserRole) or item.data(5, Qt.ItemDataRole.UserRole) or "",
        )
        self.accept()

