from __future__ import annotations

import csv
import sqlite3
from pathlib import Path


def _sqlite_ident(name: str) -> str:
    return '"' + name.replace('"', '""') + '"'


def _create_table(conn: sqlite3.Connection, table: str, headers: list[str]) -> None:
    cols = ", ".join(f"{_sqlite_ident(h)} TEXT" for h in headers)
    conn.execute(f"DROP TABLE IF EXISTS {_sqlite_ident(table)}")
    conn.execute(f"CREATE TABLE {_sqlite_ident(table)} ({cols})")


def _insert_rows(conn: sqlite3.Connection, table: str, headers: list[str], rows: list[dict[str, str]]) -> None:
    if not rows:
        return
    cols = ", ".join(_sqlite_ident(h) for h in headers)
    qmarks = ", ".join("?" for _ in headers)
    sql = f"INSERT INTO {_sqlite_ident(table)} ({cols}) VALUES ({qmarks})"
    values = [[row.get(h) or "" for h in headers] for row in rows]
    conn.executemany(sql, values)


def generate_sqlite(database_dir: Path, out_db: Path, progress_cb=None) -> None:
    csv_paths = sorted(database_dir.glob("g-*.csv"))
    conn = sqlite3.connect(out_db)
    try:
        for idx, csv_path in enumerate(csv_paths, start=1):
            with csv_path.open("r", encoding="utf-8", newline="") as handle:
                reader = csv.DictReader(handle)
                headers = list(reader.fieldnames or [])
                rows = list(reader)
            table = csv_path.stem.removeprefix("g-")
            _create_table(conn, table, headers)
            _insert_rows(conn, table, headers, rows)
            if progress_cb:
                progress_cb(idx, len(csv_paths), table)
        conn.commit()
    finally:
        conn.close()

