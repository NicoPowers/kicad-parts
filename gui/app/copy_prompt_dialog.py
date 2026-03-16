from __future__ import annotations

from PyQt6.QtWidgets import (
    QComboBox,
    QDialog,
    QDialogButtonBox,
    QFormLayout,
    QLabel,
    QVBoxLayout,
    QWidget,
)


class CopyPromptDialog(QDialog):
    def __init__(
        self,
        kind: str,
        item_name: str,
        source_path: str,
        source_provider: str,
        local_libraries: list[str],
        preview_widget: QWidget | None = None,
        parent=None,
    ):
        super().__init__(parent)
        self.setWindowTitle("Copy KiCad Item to Local Library")
        self._target_lib = ""

        info = QLabel(
            f"The selected {kind} is from '{source_provider}'.\n"
            "Copy it into a local library now?"
        )
        info.setWordWrap(True)

        self.lib_combo = QComboBox()
        self.lib_combo.addItems(local_libraries)
        if local_libraries:
            self._target_lib = local_libraries[0]
        self.lib_combo.currentTextChanged.connect(self._on_target_changed)

        form = QFormLayout()
        form.addRow("Item", QLabel(item_name))
        form.addRow("Source", QLabel(source_path))
        form.addRow("Target local library", self.lib_combo)

        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel)
        buttons.accepted.connect(self.accept)
        buttons.rejected.connect(self.reject)

        layout = QVBoxLayout(self)
        layout.addWidget(info)
        layout.addLayout(form)
        if preview_widget is not None:
            layout.addWidget(preview_widget, 1)
        layout.addWidget(buttons)

    def _on_target_changed(self, value: str) -> None:
        self._target_lib = value

    @property
    def target_library(self) -> str:
        return self._target_lib
