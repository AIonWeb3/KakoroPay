"""SQLite persistence for the AgentPay demo console."""

from __future__ import annotations

import sqlite3
from pathlib import Path

SCHEMA_PATH = Path(__file__).resolve().parent / "schema.sql"
DEFAULT_DB = Path(__file__).resolve().parent / "data" / "agentpay.db"


def connect(db_path: str | Path | None = None) -> sqlite3.Connection:
    path = Path(db_path) if db_path else DEFAULT_DB
    path.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(str(path))
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA foreign_keys = ON")
    return conn


def migrate(conn: sqlite3.Connection) -> None:
    sql = SCHEMA_PATH.read_text(encoding="utf-8")
    conn.executescript(sql)
    conn.execute("INSERT OR IGNORE INTO schema_migrations (id, name) VALUES (1, 'initial')")
    conn.commit()


def insert_session(
    conn: sqlite3.Connection,
    session_id: str,
    *,
    ledger: int = 12_400_000,
    period_ledgers: int = 17_280,
    pitch_step: int = 0,
    role: str = "operator",
) -> None:
    conn.execute(
        """
        INSERT INTO sessions (id, ledger, period_ledgers, pitch_step, role)
        VALUES (?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            ledger = excluded.ledger,
            period_ledgers = excluded.period_ledgers,
            pitch_step = excluded.pitch_step,
            role = excluded.role
        """,
        (session_id, ledger, period_ledgers, pitch_step, role),
    )
    conn.commit()


def get_session(conn: sqlite3.Connection, session_id: str) -> sqlite3.Row | None:
    return conn.execute("SELECT * FROM sessions WHERE id = ?", (session_id,)).fetchone()


def insert_policy(
    conn: sqlite3.Connection,
    session_id: str,
    spec_json: str,
    *,
    source_tx_count: int,
    period_ledgers: int,
    generated_at: str | None,
) -> int:
    cur = conn.execute(
        """
        INSERT INTO policies (session_id, spec_json, source_tx_count, period_ledgers, generated_at)
        VALUES (?, ?, ?, ?, ?)
        """,
        (session_id, spec_json, source_tx_count, period_ledgers, generated_at),
    )
    conn.commit()
    return int(cur.lastrowid)


def get_policy(conn: sqlite3.Connection, session_id: str) -> sqlite3.Row | None:
    return conn.execute(
        "SELECT * FROM policies WHERE session_id = ? ORDER BY id DESC LIMIT 1",
        (session_id,),
    ).fetchone()


def insert_rule(
    conn: sqlite3.Connection,
    *,
    rule_id: int,
    session_id: str,
    resource_id: str,
    contract_id: str,
    method: str,
    name: str,
    price: int,
    max_spend_per_period: int,
    max_calls_per_period: int,
    spent: int = 0,
    calls: int = 0,
    last_reset: int = 0,
) -> None:
    conn.execute(
        """
        INSERT INTO rules (
            rule_id, session_id, resource_id, contract_id, method, name, price,
            max_spend_per_period, max_calls_per_period, spent, calls, last_reset
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            rule_id,
            session_id,
            resource_id,
            contract_id,
            method,
            name,
            price,
            max_spend_per_period,
            max_calls_per_period,
            spent,
            calls,
            last_reset,
        ),
    )
    conn.commit()


def list_rules(conn: sqlite3.Connection, session_id: str) -> list[sqlite3.Row]:
    return conn.execute(
        "SELECT * FROM rules WHERE session_id = ? ORDER BY rule_id",
        (session_id,),
    ).fetchall()


def clear_rules(conn: sqlite3.Connection, session_id: str) -> None:
    conn.execute("DELETE FROM rules WHERE session_id = ?", (session_id,))
    conn.commit()


def update_rule_window(
    conn: sqlite3.Connection,
    session_id: str,
    resource_id: str,
    *,
    spent: int,
    calls: int,
    last_reset: int,
) -> None:
    conn.execute(
        """
        UPDATE rules
        SET spent = ?, calls = ?, last_reset = ?
        WHERE session_id = ? AND resource_id = ?
        """,
        (spent, calls, last_reset, session_id, resource_id),
    )
    conn.commit()


def clear_audit(conn: sqlite3.Connection, session_id: str) -> None:
    conn.execute("DELETE FROM audit_events WHERE session_id = ?", (session_id,))
    conn.commit()


def insert_audit(
    conn: sqlite3.Connection,
    session_id: str,
    *,
    ts: float,
    decision: str,
    reason: str,
    resource_id: str,
    amount: int,
    remaining: int,
    tx_hash: str | None = None,
    ledger: int | None = None,
) -> None:
    conn.execute(
        """
        INSERT INTO audit_events (
            session_id, ts, decision, reason, resource_id, amount, remaining, tx_hash, ledger
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (session_id, ts, decision, reason, resource_id, amount, remaining, tx_hash, ledger),
    )
    conn.commit()


def list_audit(conn: sqlite3.Connection, session_id: str, limit: int = 40) -> list[sqlite3.Row]:
    return conn.execute(
        """
        SELECT * FROM audit_events
        WHERE session_id = ?
        ORDER BY id DESC
        LIMIT ?
        """,
        (session_id, limit),
    ).fetchall()


def upsert_resource(
    conn: sqlite3.Connection,
    *,
    id: str,
    name: str,
    contract_id: str,
    method: str,
    price: int,
    description: str,
) -> None:
    conn.execute(
        """
        INSERT INTO resources (id, name, contract_id, method, price, description)
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            contract_id = excluded.contract_id,
            method = excluded.method,
            price = excluded.price,
            description = excluded.description
        """,
        (id, name, contract_id, method, price, description),
    )
    conn.commit()


def import_registry(conn: sqlite3.Connection, resources: list[dict]) -> None:
    for resource in resources:
        upsert_resource(
            conn,
            id=resource["id"],
            name=resource["name"],
            contract_id=resource["contract_id"],
            method=resource["method"],
            price=int(resource["price"]),
            description=resource.get("description", ""),
        )


def list_resources(conn: sqlite3.Connection) -> list[sqlite3.Row]:
    return conn.execute("SELECT * FROM resources ORDER BY id").fetchall()
