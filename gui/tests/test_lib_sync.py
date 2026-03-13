from __future__ import annotations

from io import BytesIO
from pathlib import Path

import pytest

from app.lib_sync import copy_footprint, copy_symbol, fetch_3d_model


class _FakeResponse:
    def __init__(self, payload: bytes):
        self._payload = payload

    def read(self) -> bytes:
        return self._payload

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        return False


def test_copy_symbol_creates_new_library(tmp_path: Path) -> None:
    src = tmp_path / "src.kicad_sym"
    src.write_text(
        '(kicad_symbol_lib\n'
        '  (version 20230121)\n'
        '  (symbol "Device:R")\n'
        ')\n',
        encoding="utf-8",
    )
    dest = tmp_path / "symbols" / "g-pas.kicad_sym"
    result = copy_symbol(src, "R", dest)
    assert result.copied is True
    text = dest.read_text(encoding="utf-8")
    assert 'symbol "g-pas:R"' in text


def test_copy_symbol_skips_duplicate(tmp_path: Path) -> None:
    src = tmp_path / "src.kicad_sym"
    src.write_text('(kicad_symbol_lib (version 20230121) (symbol "Device:R"))', encoding="utf-8")
    dest = tmp_path / "symbols" / "g-pas.kicad_sym"
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text('(kicad_symbol_lib (version 20230121) (symbol "g-pas:R"))', encoding="utf-8")
    result = copy_symbol(src, "R", dest)
    assert result.copied is False
    assert "already exists" in result.message.lower()


def test_fetch_3d_model_downloads(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    def fake_urlopen(url: str):
        assert "kicad-packages3D" in url
        return _FakeResponse(b"model-bytes")

    monkeypatch.setattr("app.lib_sync.urllib.request.urlopen", fake_urlopen)
    result = fetch_3d_model("${KICAD9_3DMODEL_DIR}/Capacitor_SMD.3dshapes/C_0603_1608Metric.step", tmp_path)
    assert result.copied is True
    assert result.path.read_bytes() == b"model-bytes"


def test_copy_footprint_rewrites_3d_reference(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    def fake_urlopen(url: str):
        return _FakeResponse(b"step-data")

    monkeypatch.setattr("app.lib_sync.urllib.request.urlopen", fake_urlopen)
    src_mod = tmp_path / "C_0603_1608Metric.kicad_mod"
    src_mod.write_text(
        '(footprint "C_0603_1608Metric"\n'
        '  (model "${KICAD9_3DMODEL_DIR}/Capacitor_SMD.3dshapes/C_0603_1608Metric.step")\n'
        ')\n',
        encoding="utf-8",
    )
    dest_pretty = tmp_path / "footprints" / "g-cap.pretty"
    local_3d = tmp_path / "3d-models"
    fp_result, model_results = copy_footprint(src_mod, dest_pretty, local_3d)
    assert fp_result.copied is True
    copied_mod = dest_pretty / src_mod.name
    text = copied_mod.read_text(encoding="utf-8")
    assert "${GITPLM_PARTS}/3d-models/C_0603_1608Metric.step" in text
    assert model_results
    assert (local_3d / "C_0603_1608Metric.step").exists()
