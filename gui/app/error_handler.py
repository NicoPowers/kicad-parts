from __future__ import annotations

import sys
import threading
import traceback
from types import TracebackType

from PyQt6.QtWidgets import (
    QApplication,
    QDialog,
    QHBoxLayout,
    QLabel,
    QMessageBox,
    QPushButton,
    QTextEdit,
    QVBoxLayout,
)


class ErrorDialog(QDialog):
    def __init__(self, title: str, message: str, details: str, parent=None):
        super().__init__(parent)
        self.setWindowTitle(title)
        self.resize(760, 420)

        message_label = QLabel(message)
        message_label.setWordWrap(True)

        self.details = QTextEdit()
        self.details.setReadOnly(True)
        self.details.setPlainText(details)

        copy_button = QPushButton("Copy Error")
        copy_button.clicked.connect(self.copy_to_clipboard)
        close_button = QPushButton("Close")
        close_button.clicked.connect(self.accept)

        button_row = QHBoxLayout()
        button_row.addStretch(1)
        button_row.addWidget(copy_button)
        button_row.addWidget(close_button)

        layout = QVBoxLayout()
        layout.addWidget(message_label)
        layout.addWidget(self.details, 1)
        layout.addLayout(button_row)
        self.setLayout(layout)

    def copy_to_clipboard(self) -> None:
        app = QApplication.instance()
        if app is None:
            return
        app.clipboard().setText(self.details.toPlainText())


def show_error_dialog(parent, title: str, message: str, details: str = "") -> None:
    dialog = ErrorDialog(title, message, details or message, parent=parent)
    dialog.exec()


def _format_exception(exctype: type[BaseException], value: BaseException, tb: TracebackType | None) -> str:
    return "".join(traceback.format_exception(exctype, value, tb))


def install_global_handler(app: QApplication) -> None:
    original_excepthook = sys.excepthook

    def handle_exception(exctype: type[BaseException], value: BaseException, tb: TracebackType | None) -> None:
        details = _format_exception(exctype, value, tb)
        try:
            show_error_dialog(None, "Unexpected error", str(value), details)
        except Exception:
            # As a final fallback, print to stderr and defer to default hook.
            pass
        original_excepthook(exctype, value, tb)

    def handle_thread_exception(args: threading.ExceptHookArgs) -> None:
        details = _format_exception(args.exc_type, args.exc_value, args.exc_traceback)
        try:
            show_error_dialog(None, "Unexpected background error", str(args.exc_value), details)
        except Exception:
            pass

    sys.excepthook = handle_exception
    threading.excepthook = handle_thread_exception

    # Keep reference to app to make sure QApplication is initialized before dialogs.
    _ = app

