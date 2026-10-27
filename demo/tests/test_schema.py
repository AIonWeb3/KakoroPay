from pathlib import Path

from store import connect, migrate

SCHEMA = Path(__file__).resolve().parents[1] / "schema.sql"


def test_schema_file_defines_core_tables():
    text = SCHEMA.read_text(encoding="utf-8")
    for table in (
        "sessions",
        "policies",
        "resources",
        "rules",
        "audit_events",
        "schema_migrations",
    ):
        assert table in text


def test_migrate_creates_tables(tmp_path):
    db = tmp_path / "test.db"
    conn = connect(db)
    migrate(conn)
    names = {
        row[0]
        for row in conn.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall()
    }
    assert "sessions" in names
    assert "schema_migrations" in names
    row = conn.execute("SELECT name FROM schema_migrations WHERE id=1").fetchone()
    assert row["name"] == "initial"
    conn.close()
