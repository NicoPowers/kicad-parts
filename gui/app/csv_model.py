from __future__ import annotations

from dataclasses import dataclass

from PyQt6.QtCore import QAbstractTableModel, QModelIndex, Qt, pyqtSignal
from PyQt6.QtGui import QColor

from .validators import find_duplicate_values, validate_cell


@dataclass
class EditRecord:
    row: int
    column: int
    before: str
    after: str


class CsvTableModel(QAbstractTableModel):
    dirtyChanged = pyqtSignal(bool)

    def __init__(self, headers: list[str], rows: list[dict[str, str]]):
        super().__init__()
        self.headers = headers
        self.rows = rows
        self._dirty = False
        self._duplicate_ipn_rows = find_duplicate_values(self.rows, "IPN")

    def rowCount(self, _parent: QModelIndex = QModelIndex()) -> int:
        return len(self.rows)

    def columnCount(self, _parent: QModelIndex = QModelIndex()) -> int:
        return len(self.headers)

    def headerData(self, section: int, orientation: Qt.Orientation, role: int = Qt.ItemDataRole.DisplayRole):
        if role != Qt.ItemDataRole.DisplayRole:
            return None
        if orientation == Qt.Orientation.Horizontal:
            return self.headers[section]
        return section + 1

    def data(self, index: QModelIndex, role: int = Qt.ItemDataRole.DisplayRole):
        if not index.isValid():
            return None
        row = self.rows[index.row()]
        column = self.headers[index.column()]
        value = row.get(column, "")
        if role in (Qt.ItemDataRole.DisplayRole, Qt.ItemDataRole.EditRole):
            return value
        if role == Qt.ItemDataRole.ForegroundRole and column == "IPN" and index.row() in self._duplicate_ipn_rows:
            return QColor("#b00020")
        if role == Qt.ItemDataRole.BackgroundRole and validate_cell(column, value):
            return QColor("#fff4f4")
        return None

    def flags(self, index: QModelIndex):
        base = super().flags(index)
        if not index.isValid():
            return base
        return base | Qt.ItemFlag.ItemIsEditable | Qt.ItemFlag.ItemIsSelectable | Qt.ItemFlag.ItemIsEnabled

    def setData(self, index: QModelIndex, value, role: int = Qt.ItemDataRole.EditRole):
        if role != Qt.ItemDataRole.EditRole or not index.isValid():
            return False
        row = self.rows[index.row()]
        column = self.headers[index.column()]
        before = row.get(column, "")
        after = str(value)
        if before == after:
            return False
        row[column] = after
        self._duplicate_ipn_rows = find_duplicate_values(self.rows, "IPN")
        self.dataChanged.emit(index, index, [Qt.ItemDataRole.DisplayRole])
        self.set_dirty(True)
        return True

    def set_cell(self, row: int, column: int, value: str) -> None:
        model_index = self.index(row, column)
        self.setData(model_index, value)

    def insert_row(self, row_data: dict[str, str]) -> int:
        row_idx = len(self.rows)
        self.beginInsertRows(QModelIndex(), row_idx, row_idx)
        self.rows.append(row_data)
        self.endInsertRows()
        self._duplicate_ipn_rows = find_duplicate_values(self.rows, "IPN")
        self.set_dirty(True)
        return row_idx

    def delete_row(self, row_idx: int) -> dict[str, str]:
        row = self.rows[row_idx]
        self.beginRemoveRows(QModelIndex(), row_idx, row_idx)
        self.rows.pop(row_idx)
        self.endRemoveRows()
        self._duplicate_ipn_rows = find_duplicate_values(self.rows, "IPN")
        self.set_dirty(True)
        return row

    def duplicate_row(self, row_idx: int) -> int:
        return self.insert_row(self.rows[row_idx].copy())

    def sort(self, column: int, order: Qt.SortOrder = Qt.SortOrder.AscendingOrder):
        key = self.headers[column]
        reverse = order == Qt.SortOrder.DescendingOrder
        self.layoutAboutToBeChanged.emit()
        self.rows.sort(key=lambda row: row.get(key, ""), reverse=reverse)
        self.layoutChanged.emit()
        self._duplicate_ipn_rows = find_duplicate_values(self.rows, "IPN")
        self.set_dirty(True)

    @property
    def dirty(self) -> bool:
        return self._dirty

    def set_dirty(self, is_dirty: bool) -> None:
        if self._dirty == is_dirty:
            return
        self._dirty = is_dirty
        self.dirtyChanged.emit(is_dirty)

