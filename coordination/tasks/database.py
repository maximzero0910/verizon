"""Transactional SQLite storage for the J2H agent task board."""

from __future__ import annotations

import json
import sqlite3
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterator


SCHEMA_VERSION = 1
AGENTS = ("codex", "big-pickle", "copilot")
ACTORS = (*AGENTS, "user")
PRIORITIES = ("critical", "high", "medium", "low")
STATUSES = ("open", "in_progress", "blocked", "review", "completed", "cancelled")
TERMINAL_STATUSES = {"completed", "cancelled"}
STATUS_TRANSITIONS = {
    "open": {"in_progress", "cancelled"},
    "in_progress": {"open", "blocked", "review", "cancelled"},
    "blocked": {"in_progress", "cancelled"},
    "review": {"in_progress", "cancelled"},
    "completed": set(),
    "cancelled": set(),
}


class TaskError(ValueError):
    """An invalid task operation or database state."""


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def validate_actor(actor: str) -> None:
    if actor not in ACTORS:
        raise TaskError(f"unknown actor {actor!r}; expected one of: {', '.join(ACTORS)}")


class TaskDatabase:
    def __init__(self, path: str | Path, busy_timeout_ms: int = 5000):
        self.path = Path(path)
        self.busy_timeout_ms = busy_timeout_ms

    @staticmethod
    def _run_schema(connection: sqlite3.Connection, schema_path: Path) -> None:
        statement = ""
        for line in schema_path.read_text(encoding="utf-8").splitlines(keepends=True):
            statement += line
            if sqlite3.complete_statement(statement):
                sql = statement.strip()
                if sql:
                    connection.execute(sql)
                statement = ""
        if statement.strip():
            raise TaskError("schema.sql ends with an incomplete SQL statement")

    def initialize(self, schema_path: str | Path | None = None) -> None:
        """Create a new database and schema; fail without touching any existing path."""
        self.path.parent.mkdir(parents=True, exist_ok=True)
        try:
            descriptor = self.path.open("xb")
        except FileExistsError as error:
            raise TaskError(f"database already exists; refusing to overwrite: {self.path}") from error
        descriptor.close()
        schema_file = Path(schema_path) if schema_path else Path(__file__).with_name("schema.sql")
        try:
            connection = sqlite3.connect(self.path, timeout=self.busy_timeout_ms / 1000)
            try:
                connection.execute(f"PRAGMA busy_timeout = {int(self.busy_timeout_ms)}")
                connection.execute("PRAGMA foreign_keys = ON")
                connection.execute("BEGIN EXCLUSIVE")
                self._run_schema(connection, schema_file)
                connection.execute(f"PRAGMA user_version = {SCHEMA_VERSION}")
                connection.commit()
            except Exception:
                connection.rollback()
                raise
            finally:
                connection.close()
        except Exception:
            self.path.unlink(missing_ok=True)
            raise

    @contextmanager
    def transaction(self, *, write: bool = False) -> Iterator[sqlite3.Connection]:
        if not self.path.is_file():
            raise TaskError(f"task database does not exist: {self.path}; run init first")
        connection = sqlite3.connect(self.path, timeout=self.busy_timeout_ms / 1000)
        connection.row_factory = sqlite3.Row
        connection.execute(f"PRAGMA busy_timeout = {int(self.busy_timeout_ms)}")
        connection.execute("PRAGMA foreign_keys = ON")
        try:
            version = connection.execute("PRAGMA user_version").fetchone()[0]
            if version != SCHEMA_VERSION:
                raise TaskError(
                    f"unsupported task database schema version {version}; expected {SCHEMA_VERSION}"
                )
            connection.execute("BEGIN IMMEDIATE" if write else "BEGIN")
            yield connection
            connection.commit()
        except Exception:
            connection.rollback()
            raise
        finally:
            connection.close()

    @staticmethod
    def _event(connection, task_id, event_type, actor, message, details=None):
        connection.execute(
            "INSERT INTO task_events(task_id,event_type,actor,timestamp,message,details_json) "
            "VALUES(?,?,?,?,?,?)",
            (task_id, event_type, actor, utc_now(), message, json.dumps(details or {}, sort_keys=True)),
        )

    @staticmethod
    def _task(connection, task_id):
        row = connection.execute("SELECT * FROM tasks WHERE id=?", (task_id,)).fetchone()
        if row is None:
            raise TaskError(f"task {task_id} does not exist")
        return row

    @staticmethod
    def _authorize(task, actor: str):
        validate_actor(actor)
        if actor not in (task["assigned_agent"], "codex"):
            raise TaskError(f"only {task['assigned_agent']} or codex may update task {task['id']}")

    @staticmethod
    def _dependency(connection, task_id: int, dependency_id: int) -> None:
        if task_id == dependency_id:
            raise TaskError("a task cannot depend on itself")
        if connection.execute("SELECT 1 FROM tasks WHERE id=?", (dependency_id,)).fetchone() is None:
            raise TaskError(f"dependency task {dependency_id} does not exist")
        cycle = connection.execute(
            "WITH RECURSIVE depends(id) AS ("
            " SELECT depends_on FROM task_dependencies WHERE task_id=?"
            " UNION SELECT d.depends_on FROM task_dependencies d JOIN depends x ON d.task_id=x.id"
            ") SELECT 1 FROM depends WHERE id=? LIMIT 1",
            (dependency_id, task_id),
        ).fetchone()
        if cycle:
            raise TaskError(f"dependency {dependency_id} would create a circular dependency")

    def create(self, *, title, description, agent, actor, priority="medium", deadline=None, dependencies=()):
        validate_actor(actor)
        if agent not in AGENTS:
            raise TaskError(f"unknown agent {agent!r}; expected one of: {', '.join(AGENTS)}")
        if priority not in PRIORITIES:
            raise TaskError(f"invalid priority {priority!r}; expected one of: {', '.join(PRIORITIES)}")
        if not title.strip() or not description.strip():
            raise TaskError("title and description are required")
        now = utc_now()
        with self.transaction(write=True) as connection:
            cursor = connection.execute(
                "INSERT INTO tasks(title,description,assigned_agent,created_by,priority,status,created_at,updated_at,deadline) "
                "VALUES(?,?,?,?,?,'open',?,?,?)",
                (title.strip(), description.strip(), agent, actor, priority, now, now, deadline),
            )
            task_id = cursor.lastrowid
            self._event(connection, task_id, "created", actor, f"Created and assigned to {agent}.",
                        {"assigned_agent": agent, "priority": priority})
            for dependency_id in dependencies:
                self._dependency(connection, task_id, dependency_id)
                connection.execute(
                    "INSERT INTO task_dependencies(task_id,depends_on) VALUES(?,?)", (task_id, dependency_id)
                )
                self._event(connection, task_id, "dependency_added", actor,
                            f"Added dependency on task {dependency_id}.", {"depends_on": dependency_id})
            return task_id

    def add_dependency(self, task_id: int, dependency_id: int, actor: str):
        validate_actor(actor)
        with self.transaction(write=True) as connection:
            task = self._task(connection, task_id)
            self._authorize(task, actor)
            if task["status"] in TERMINAL_STATUSES:
                raise TaskError(f"cannot add dependencies to a {task['status']} task")
            self._dependency(connection, task_id, dependency_id)
            try:
                connection.execute("INSERT INTO task_dependencies(task_id,depends_on) VALUES(?,?)",
                                   (task_id, dependency_id))
            except sqlite3.IntegrityError as error:
                raise TaskError(f"task {task_id} already depends on task {dependency_id}") from error
            self._event(connection, task_id, "dependency_added", actor,
                        f"Added dependency on task {dependency_id}.", {"depends_on": dependency_id})

    def claim(self, task_id: int, actor: str):
        validate_actor(actor)
        with self.transaction(write=True) as connection:
            task = self._task(connection, task_id)
            self._authorize(task, actor)
            if task["status"] != "open":
                raise TaskError(f"only open tasks can be claimed (task is {task['status']})")
            self._change_status(connection, task, "in_progress", actor, "Claimed task.")

    @staticmethod
    def _change_status(connection, task, status, actor, message):
        old = task["status"]
        if status not in STATUS_TRANSITIONS[old]:
            raise TaskError(f"status cannot change from {old} to {status}")
        now = utc_now()
        connection.execute("UPDATE tasks SET status=?,updated_at=? WHERE id=?", (status, now, task["id"]))
        TaskDatabase._event(connection, task["id"], "status_changed", actor, message,
                            {"from": old, "to": status})

    def update(self, task_id: int, actor: str, *, status=None, priority=None, blocker=None):
        validate_actor(actor)
        if status is not None and status not in STATUSES:
            raise TaskError(f"invalid status {status!r}; expected one of: {', '.join(STATUSES)}")
        if priority is not None and priority not in PRIORITIES:
            raise TaskError(f"invalid priority {priority!r}; expected one of: {', '.join(PRIORITIES)}")
        if blocker is not None and not blocker.strip():
            raise TaskError("blocker message cannot be empty")
        if status == "blocked" and blocker is None:
            raise TaskError("setting status to blocked requires a --blocker explanation")
        if status == "completed":
            raise TaskError("use complete with result and verification evidence")
        with self.transaction(write=True) as connection:
            task = self._task(connection, task_id)
            self._authorize(task, actor)
            if blocker is not None:
                if status not in (None, "blocked"):
                    raise TaskError("a blocker can only be recorded while setting status to blocked")
                if status is None:
                    status = "blocked"
                self._event(connection, task_id, "blocker", actor, blocker.strip())
            if status is not None and status != task["status"]:
                self._change_status(connection, task, status, actor, f"Status changed to {status}.")
                task = dict(task)
                task["status"] = status
            if priority is not None and priority != task["priority"]:
                connection.execute("UPDATE tasks SET priority=?,updated_at=? WHERE id=?",
                                   (priority, utc_now(), task_id))
                self._event(connection, task_id, "priority_changed", actor,
                            f"Priority changed to {priority}.", {"from": task["priority"], "to": priority})

    def assign(self, task_id: int, agent: str, actor: str):
        validate_actor(actor)
        if actor != "codex":
            raise TaskError("only codex may reassign tasks")
        if agent not in AGENTS:
            raise TaskError(f"unknown agent {agent!r}; expected one of: {', '.join(AGENTS)}")
        with self.transaction(write=True) as connection:
            task = self._task(connection, task_id)
            if task["assigned_agent"] == agent:
                return
            connection.execute("UPDATE tasks SET assigned_agent=?,updated_at=? WHERE id=?",
                               (agent, utc_now(), task_id))
            self._event(connection, task_id, "assignment_changed", actor,
                        f"Reassigned from {task['assigned_agent']} to {agent}.",
                        {"from": task["assigned_agent"], "to": agent})

    def comment(self, task_id: int, actor: str, message: str):
        validate_actor(actor)
        if not message.strip():
            raise TaskError("comment cannot be empty")
        with self.transaction(write=True) as connection:
            task = self._task(connection, task_id)
            if actor not in (task["assigned_agent"], "codex") and not (
                actor == "copilot" and task["status"] == "review"
            ):
                raise TaskError(f"{actor} is not allowed to comment on task {task_id}")
            connection.execute("UPDATE tasks SET updated_at=? WHERE id=?", (utc_now(), task_id))
            self._event(connection, task_id, "comment", actor, message.strip())

    def review(self, task_id: int, actor: str, decision: str, message: str):
        if actor != "copilot":
            raise TaskError("only copilot may record review decisions")
        if decision not in ("approve", "request_changes"):
            raise TaskError("review decision must be approve or request_changes")
        if not message.strip():
            raise TaskError("review decision requires a message")
        with self.transaction(write=True) as connection:
            task = self._task(connection, task_id)
            if task["status"] != "review":
                raise TaskError("review decisions require task status 'review'")
            connection.execute("UPDATE tasks SET updated_at=? WHERE id=?", (utc_now(), task_id))
            self._event(connection, task_id, "review_decision", actor, message.strip(),
                        {"decision": decision})

    def complete(self, task_id: int, actor: str, result: str, evidence: str):
        if actor != "codex":
            raise TaskError("only codex may mark a task completed after verification")
        if not result.strip() or not evidence.strip():
            raise TaskError("completion requires both --result and --evidence")
        with self.transaction(write=True) as connection:
            task = self._task(connection, task_id)
            if task["status"] not in ("in_progress", "review"):
                raise TaskError(f"only in_progress or review tasks can be completed (task is {task['status']})")
            unresolved = connection.execute(
                "SELECT d.depends_on FROM task_dependencies d JOIN tasks t ON t.id=d.depends_on "
                "WHERE d.task_id=? AND t.status!='completed' ORDER BY d.depends_on", (task_id,)
            ).fetchall()
            if unresolved:
                ids = ", ".join(str(row[0]) for row in unresolved)
                raise TaskError(f"cannot complete task while dependencies are unresolved: {ids}")
            connection.execute(
                "UPDATE tasks SET status='completed',completion_summary=?,verification_evidence=?,updated_at=? WHERE id=?",
                (result.strip(), evidence.strip(), utc_now(), task_id),
            )
            self._event(connection, task_id, "status_changed", actor, "Status changed to completed.",
                        {"from": task["status"], "to": "completed"})
            self._event(connection, task_id, "completed", actor, result.strip(), {"evidence": evidence.strip()})

    def get(self, task_id: int):
        with self.transaction() as connection:
            task = dict(self._task(connection, task_id))
            task["dependencies"] = [row[0] for row in connection.execute(
                "SELECT depends_on FROM task_dependencies WHERE task_id=? ORDER BY depends_on", (task_id,)
            )]
            task["dependents"] = [row[0] for row in connection.execute(
                "SELECT task_id FROM task_dependencies WHERE depends_on=? ORDER BY task_id", (task_id,)
            )]
            return task

    def list(self, *, agent=None, status=None):
        if agent is not None and agent not in AGENTS:
            raise TaskError(f"unknown agent {agent!r}; expected one of: {', '.join(AGENTS)}")
        if status is not None and status not in STATUSES:
            raise TaskError(f"invalid status {status!r}; expected one of: {', '.join(STATUSES)}")
        clauses, parameters = [], []
        if agent:
            clauses.append("assigned_agent=?")
            parameters.append(agent)
        if status:
            clauses.append("status=?")
            parameters.append(status)
        query = "SELECT * FROM tasks" + (" WHERE " + " AND ".join(clauses) if clauses else "")
        query += " ORDER BY CASE priority WHEN 'critical' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 ELSE 3 END, id"
        with self.transaction() as connection:
            return [dict(row) for row in connection.execute(query, parameters)]

    def history(self, task_id: int):
        with self.transaction() as connection:
            self._task(connection, task_id)
            return [dict(row) for row in connection.execute(
                "SELECT * FROM task_events WHERE task_id=? ORDER BY id", (task_id,)
            )]
