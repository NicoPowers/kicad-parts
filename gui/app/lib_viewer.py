from __future__ import annotations

import re
import sys
from pathlib import Path
from typing import Any, Iterable

from PyQt6.QtCore import QPointF, QRectF, Qt
from PyQt6.QtGui import QColor, QFont, QFontMetricsF, QPainter, QPainterPath, QPen
from PyQt6.QtWidgets import QLabel, QVBoxLayout, QWidget

from .symbol_units import item_matches_variant, unit_options_from_symbol

def _ensure_kicad_utils_on_path(workspace_root: Path) -> None:
    common = workspace_root / "libs" / "kicad-library-utils" / "common"
    if common.exists() and str(common) not in sys.path:
        sys.path.insert(0, str(common))


class _BaseViewer(QWidget):
    def __init__(self, title: str, parent=None):
        super().__init__(parent)
        self._title = QLabel(title)
        self._title.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self._status = QLabel("No item selected")
        self._status.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self._status.setWordWrap(True)
        self._status.setStyleSheet("color: #666;")

        layout = QVBoxLayout(self)
        layout.setContentsMargins(6, 6, 6, 6)
        layout.addWidget(self._title)
        layout.addWidget(self._status)

        self._bbox = QRectF(-10, -10, 20, 20)

    def _set_status(self, text: str) -> None:
        self._status.setText(text)

    def _make_transform(self) -> tuple[float, float, float]:
        r = self.rect().adjusted(8, 36, -8, -8)
        width = max(self._bbox.width(), 1.0)
        height = max(self._bbox.height(), 1.0)
        sx = r.width() / width
        sy = r.height() / height
        scale = max(0.1, min(sx, sy))
        tx = r.center().x() - (self._bbox.left() + width / 2.0) * scale
        ty = r.center().y() + (self._bbox.top() + height / 2.0) * scale
        return scale, tx, ty

    def _map(self, x: float, y: float) -> QPointF:
        scale, tx, ty = self._make_transform()
        # Invert Y so symbol orientation is natural in screen coordinates.
        return QPointF(x * scale + tx, -y * scale + ty)


class FootprintViewer(_BaseViewer):
    def __init__(self, workspace_root: Path, parent=None):
        super().__init__("Footprint Preview", parent=parent)
        self.workspace_root = workspace_root
        self.kmod = None

    def clear(self) -> None:
        self.kmod = None
        self._set_status("No item selected")
        self.update()

    def load_file(self, path: Path) -> None:
        _ensure_kicad_utils_on_path(self.workspace_root)
        try:
            from kicad_mod import KicadMod  # type: ignore
        except Exception as exc:
            self.kmod = None
            self._set_status(f"kicad-library-utils missing: {exc}")
            self.update()
            return
        try:
            self.kmod = KicadMod(filename=str(path))
            self._recompute_bbox()
            self._set_status(path.name)
        except Exception as exc:
            self.kmod = None
            self._set_status(f"Unable to parse footprint: {exc}")
        self.update()

    def _iter_points(self) -> Iterable[tuple[float, float]]:
        if not self.kmod:
            return []
        points: list[tuple[float, float]] = []
        for line in self.kmod.lines:
            points.append((line["start"]["x"], line["start"]["y"]))
            points.append((line["end"]["x"], line["end"]["y"]))
        for circle in self.kmod.circles:
            points.append((circle["center"]["x"], circle["center"]["y"]))
            points.append((circle["end"]["x"], circle["end"]["y"]))
        for poly in self.kmod.polys:
            for pt in poly["points"]:
                points.append((pt["x"], pt["y"]))
        for arc in self.kmod.arcs:
            points.append((arc["start"]["x"], arc["start"]["y"]))
            points.append((arc["mid"]["x"], arc["mid"]["y"]))
            points.append((arc["end"]["x"], arc["end"]["y"]))
        for pad in self.kmod.pads:
            points.append((pad["pos"]["x"], pad["pos"]["y"]))
        return points

    def _recompute_bbox(self) -> None:
        points = list(self._iter_points())
        if not points:
            self._bbox = QRectF(-10, -10, 20, 20)
            return
        xs = [p[0] for p in points]
        ys = [p[1] for p in points]
        self._bbox = QRectF(min(xs), min(ys), max(max(xs) - min(xs), 1.0), max(max(ys) - min(ys), 1.0))

    def paintEvent(self, event) -> None:  # type: ignore[override]
        super().paintEvent(event)
        if not self.kmod:
            return
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing, True)

        scale, _, _ = self._make_transform()

        # Pads
        pad_rects: list[tuple[QRectF, str]] = []
        for pad in self.kmod.pads:
            center = self._map(pad["pos"]["x"], pad["pos"]["y"])
            size = pad.get("size", {"x": 0.8, "y": 0.8})
            w = max(1.0, size.get("x", 0.8) * scale)
            h = max(1.0, size.get("y", 0.8) * scale)
            rect = QRectF(center.x() - w / 2.0, center.y() - h / 2.0, w, h)
            color = QColor("#cc3344") if pad.get("type") == "smd" else QColor("#2e8b57")
            painter.setPen(QPen(color, 1.2))
            painter.setBrush(color.lighter(160))
            painter.drawRoundedRect(rect, 2, 2)
            pad_rects.append((rect, str(pad.get("number", ""))))

        # Pad numbers
        pad_count = len(pad_rects)
        font_pt = max(4.0, min(10.0, 0.6 * scale))
        if font_pt >= 4.5 and pad_count <= 120:
            font = QFont()
            font.setPointSizeF(font_pt)
            painter.setFont(font)
            painter.setPen(QPen(QColor("#ffffff"), 1.0))
            fm = QFontMetricsF(font)
            for rect, number in pad_rects:
                if not number:
                    continue
                tw = fm.horizontalAdvance(number)
                th = fm.height()
                if tw < rect.width() * 1.6 and th < rect.height() * 1.6:
                    painter.drawText(
                        rect.center() + QPointF(-tw / 2.0, th / 4.0), number
                    )

        silk_pen = QPen(QColor("#f2d16b"), 1.0)
        painter.setPen(silk_pen)
        painter.setBrush(Qt.BrushStyle.NoBrush)

        for line in self.kmod.lines:
            painter.drawLine(
                self._map(line["start"]["x"], line["start"]["y"]),
                self._map(line["end"]["x"], line["end"]["y"]),
            )
        for circle in self.kmod.circles:
            c = self._map(circle["center"]["x"], circle["center"]["y"])
            e = self._map(circle["end"]["x"], circle["end"]["y"])
            radius = max(1.0, ((c.x() - e.x()) ** 2 + (c.y() - e.y()) ** 2) ** 0.5)
            painter.drawEllipse(c, radius, radius)
        for poly in self.kmod.polys:
            pts = [self._map(pt["x"], pt["y"]) for pt in poly["points"]]
            for i in range(1, len(pts)):
                painter.drawLine(pts[i - 1], pts[i])
        for arc in self.kmod.arcs:
            start = self._map(arc["start"]["x"], arc["start"]["y"])
            mid = self._map(arc["mid"]["x"], arc["mid"]["y"])
            end = self._map(arc["end"]["x"], arc["end"]["y"])
            path = QPainterPath(start)
            path.quadTo(mid, end)
            painter.drawPath(path)


class SymbolViewer(_BaseViewer):
    def __init__(self, workspace_root: Path, parent=None):
        super().__init__("Symbol Preview", parent=parent)
        self.workspace_root = workspace_root
        self.symbol = None
        self._selected_unit = 1
        self._selected_demorgan = 1
        self._visible_rectangles: list[Any] = []
        self._visible_circles: list[Any] = []
        self._visible_arcs: list[Any] = []
        self._visible_polylines: list[Any] = []
        self._visible_pins: list[Any] = []

    def clear(self) -> None:
        self.symbol = None
        self._selected_unit = 1
        self._selected_demorgan = 1
        self._clear_visible_items()
        self._set_status("No item selected")
        self.update()

    @property
    def selected_unit(self) -> int:
        return self._selected_unit

    def unit_options(self) -> list[tuple[int, str]]:
        if not self.symbol:
            return []
        return unit_options_from_symbol(self.symbol)

    def has_multiple_units(self) -> bool:
        return len(self.unit_options()) > 1

    def set_selected_unit(self, unit: int) -> None:
        if not self.symbol:
            return
        valid_units = {option[0] for option in self.unit_options()}
        if not valid_units:
            return
        if unit not in valid_units:
            unit = min(valid_units)
        if unit == self._selected_unit:
            return
        self._selected_unit = unit
        self._refresh_visible_items()
        self._recompute_bbox()
        self.update()

    def load_symbol(self, symbol_file: Path, symbol_name: str) -> None:
        _ensure_kicad_utils_on_path(self.workspace_root)
        try:
            from kicad_sym import KicadLibrary  # type: ignore
        except Exception as exc:
            self.symbol = None
            self._set_status(f"kicad-library-utils missing: {exc}")
            self.update()
            return
        try:
            library = self._load_symbol_library(KicadLibrary, symbol_file)
            self.symbol = library.get_symbol(symbol_name.split(":")[-1])
            if not self.symbol:
                self._clear_visible_items()
                self._set_status(f"Symbol not found: {symbol_name}")
            else:
                self._selected_unit = 1
                self._selected_demorgan = 1
                self._refresh_visible_items()
                self._set_status(f"{symbol_file.stem}:{self.symbol.name}")
                self._recompute_bbox()
        except Exception as exc:
            self.symbol = None
            self._clear_visible_items()
            self._set_status(f"Unable to parse symbol: {exc}")
        self.update()

    def _load_symbol_library(self, kicad_library_cls, symbol_file: Path):
        """
        Parse symbol libraries with version compatibility fallback.

        kicad-library-utils can lag behind KiCad symbol file format version bumps.
        If we hit a strict version mismatch, retry by patching the top-level
        `(version NNNNNNNN)` token to the parser's expected value.
        """
        try:
            return kicad_library_cls.from_file(str(symbol_file), check_inheritance=False)
        except Exception as exc:
            message = str(exc)
            if "Version of symbol file is" not in message:
                raise
            expected = getattr(kicad_library_cls, "version", None)
            if not expected:
                raise
            text = symbol_file.read_text(encoding="utf-8", errors="ignore")
            patched = re.sub(r"\(version\s+\d+\)", f"(version {expected})", text, count=1)
            return kicad_library_cls.from_file(str(symbol_file), data=patched, check_inheritance=False)

    def _iter_points(self) -> Iterable[tuple[float, float]]:
        if not self.symbol:
            return []
        points: list[tuple[float, float]] = []
        for rect in self._visible_rectangles:
            points.extend([(rect.startx, rect.starty), (rect.endx, rect.endy)])
        for circle in self._visible_circles:
            points.extend([(circle.centerx - circle.radius, circle.centery - circle.radius)])
            points.extend([(circle.centerx + circle.radius, circle.centery + circle.radius)])
        for arc in self._visible_arcs:
            points.extend([(arc.startx, arc.starty), (arc.midx, arc.midy), (arc.endx, arc.endy)])
        for poly in self._visible_polylines:
            for pt in poly.points:
                points.append((pt.x, pt.y))
        for pin in self._visible_pins:
            points.append((pin.posx, pin.posy))
            if pin.rotation == 0:
                points.append((pin.posx + pin.length, pin.posy))
            elif pin.rotation == 180:
                points.append((pin.posx - pin.length, pin.posy))
            elif pin.rotation == 90:
                points.append((pin.posx, pin.posy + pin.length))
            elif pin.rotation == 270:
                points.append((pin.posx, pin.posy - pin.length))
        return points

    def _clear_visible_items(self) -> None:
        self._visible_rectangles = []
        self._visible_circles = []
        self._visible_arcs = []
        self._visible_polylines = []
        self._visible_pins = []

    def _refresh_visible_items(self) -> None:
        if not self.symbol:
            self._clear_visible_items()
            return
        self._visible_rectangles = [
            item for item in self.symbol.rectangles if item_matches_variant(item, self._selected_unit, self._selected_demorgan)
        ]
        self._visible_circles = [
            item for item in self.symbol.circles if item_matches_variant(item, self._selected_unit, self._selected_demorgan)
        ]
        self._visible_arcs = [
            item for item in self.symbol.arcs if item_matches_variant(item, self._selected_unit, self._selected_demorgan)
        ]
        self._visible_polylines = [
            item for item in self.symbol.polylines if item_matches_variant(item, self._selected_unit, self._selected_demorgan)
        ]
        self._visible_pins = [
            item for item in self.symbol.pins if item_matches_variant(item, self._selected_unit, self._selected_demorgan)
        ]

    def _recompute_bbox(self) -> None:
        points = list(self._iter_points())
        if not points:
            self._bbox = QRectF(-10, -10, 20, 20)
            return
        xs = [p[0] for p in points]
        ys = [p[1] for p in points]
        self._bbox = QRectF(min(xs), min(ys), max(max(xs) - min(xs), 1.0), max(max(ys) - min(ys), 1.0))

    def paintEvent(self, event) -> None:  # type: ignore[override]
        super().paintEvent(event)
        if not self.symbol:
            return
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing, True)

        body_pen = QPen(QColor("#f2d16b"), 1.2)
        pin_pen = QPen(QColor("#7ccf7c"), 1.1)
        text_pen = QPen(QColor("#76c7f2"), 1.0)

        painter.setPen(body_pen)
        painter.setBrush(Qt.BrushStyle.NoBrush)
        for rect in self._visible_rectangles:
            p1 = self._map(rect.startx, rect.starty)
            p2 = self._map(rect.endx, rect.endy)
            painter.drawRect(QRectF(p1, p2).normalized())
        for circle in self._visible_circles:
            c = self._map(circle.centerx, circle.centery)
            scale, _, _ = self._make_transform()
            radius = max(1.0, circle.radius * scale)
            painter.drawEllipse(c, radius, radius)
        for poly in self._visible_polylines:
            pts = [self._map(pt.x, pt.y) for pt in poly.points]
            for i in range(1, len(pts)):
                painter.drawLine(pts[i - 1], pts[i])
        for arc in self._visible_arcs:
            start = self._map(arc.startx, arc.starty)
            mid = self._map(arc.midx, arc.midy)
            end = self._map(arc.endx, arc.endy)
            path = QPainterPath(start)
            path.quadTo(mid, end)
            painter.drawPath(path)

        painter.setPen(pin_pen)
        for pin in self._visible_pins:
            p0 = self._map(pin.posx, pin.posy)
            if pin.rotation == 0:
                p1 = self._map(pin.posx + pin.length, pin.posy)
            elif pin.rotation == 180:
                p1 = self._map(pin.posx - pin.length, pin.posy)
            elif pin.rotation == 90:
                p1 = self._map(pin.posx, pin.posy + pin.length)
            else:
                p1 = self._map(pin.posx, pin.posy - pin.length)
            painter.drawLine(p0, p1)

        scale, _, _ = self._make_transform()
        font_pt = max(4.0, min(10.0, 1.27 * scale))
        pin_count = len(self._visible_pins)
        show_labels = font_pt >= 4.5 and pin_count <= 80

        if show_labels:
            font = QFont()
            font.setPointSizeF(font_pt)
            painter.setFont(font)
            painter.setPen(text_pen)
            fm = QFontMetricsF(font)
            pad = max(2.0, font_pt * 0.3)
            for pin in self._visible_pins:
                label = pin.number
                p = self._map(pin.posx, pin.posy)
                if pin.rotation == 0:
                    painter.drawText(p + QPointF(-fm.horizontalAdvance(label) - pad, font_pt * 0.35), label)
                elif pin.rotation == 180:
                    painter.drawText(p + QPointF(pad, font_pt * 0.35), label)
                elif pin.rotation == 90:
                    painter.drawText(p + QPointF(pad, -pad), label)
                else:
                    painter.drawText(p + QPointF(pad, font_pt + pad), label)
