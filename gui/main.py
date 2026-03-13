from __future__ import annotations

import sys
from pathlib import Path

from PyQt6.QtWidgets import QApplication

from app.error_handler import install_global_handler
from app.main_window import MainWindow


def main() -> int:
    app = QApplication(sys.argv)
    install_global_handler(app)
    workspace_root = Path(__file__).resolve().parents[1]
    window = MainWindow(workspace_root)
    window.show()
    return app.exec()


if __name__ == "__main__":
    raise SystemExit(main())
