import concurrent.futures
import json
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from database import TaskDatabase, TaskError


class TaskDatabaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.path = Path(self.temp.name) / "tasks.db"
        self.db = TaskDatabase(self.path)
        self.db.initialize()

    def tearDown(self):
        self.temp.cleanup()

    def make_task(self, title="Task", agent="codex"):
        return self.db.create(title=title, description="Detailed task description", agent=agent,
                              actor="codex", priority="high")

    def test_initialization_is_versioned_and_never_overwrites(self):
        with sqlite3.connect(self.path) as connection:
            self.assertEqual(connection.execute("PRAGMA user_version").fetchone()[0], 1)
            self.assertEqual(connection.execute("PRAGMA foreign_keys").fetchone()[0], 0)
        original = self.path.read_bytes()
        with self.assertRaisesRegex(TaskError, "refusing to overwrite"):
            self.db.initialize()
        self.assertEqual(self.path.read_bytes(), original)
        with self.db.transaction() as connection:
            self.assertEqual(connection.execute("PRAGMA foreign_keys").fetchone()[0], 1)
            self.assertEqual(connection.execute("PRAGMA busy_timeout").fetchone()[0], 5000)

    def test_task_creation_assignment_and_invalid_values(self):
        task_id = self.make_task(agent="big-pickle")
        task = self.db.get(task_id)
        self.assertEqual((task["assigned_agent"], task["created_by"], task["status"]),
                         ("big-pickle", "codex", "open"))
        with self.assertRaisesRegex(TaskError, "unknown agent"):
            self.db.create(title="Bad", description="Description", agent="intruder", actor="codex")
        with self.assertRaisesRegex(TaskError, "invalid priority"):
            self.db.create(title="Bad", description="Description", agent="codex", actor="codex",
                           priority="urgent")
        with self.assertRaisesRegex(TaskError, "only big-pickle or codex"):
            self.db.claim(task_id, "copilot")
        self.db.claim(task_id, "big-pickle")
        self.assertEqual(self.db.get(task_id)["status"], "in_progress")

    def test_status_validation_and_history_are_preserved(self):
        task_id = self.make_task()
        with self.assertRaisesRegex(TaskError, "invalid status"):
            self.db.update(task_id, "codex", status="done")
        self.db.claim(task_id, "codex")
        self.db.comment(task_id, "codex", "Implementation started")
        self.db.update(task_id, "codex", priority="critical")
        with self.assertRaisesRegex(TaskError, "requires a --blocker"):
            self.db.update(task_id, "codex", status="blocked")
        self.db.update(task_id, "codex", status="blocked", blocker="Waiting for API evidence")
        self.db.update(task_id, "codex", status="in_progress")
        kinds = [event["event_type"] for event in self.db.history(task_id)]
        self.assertEqual(kinds, ["created", "status_changed", "comment", "priority_changed", "blocker", "status_changed",
                                 "status_changed"])

    def test_dependencies_reject_missing_and_circular_links(self):
        first = self.make_task("First")
        second = self.make_task("Second", "copilot")
        with self.assertRaisesRegex(TaskError, "does not exist"):
            self.db.add_dependency(second, 999, "copilot")
        self.db.add_dependency(second, first, "copilot")
        with self.assertRaisesRegex(TaskError, "circular dependency"):
            self.db.add_dependency(first, second, "codex")
        self.assertEqual(self.db.get(second)["dependencies"], [first])

    def test_completion_requires_evidence_and_resolved_dependencies(self):
        dependency = self.make_task("Dependency")
        task = self.make_task("Dependent")
        self.db.add_dependency(task, dependency, "codex")
        self.db.claim(task, "codex")
        with self.assertRaisesRegex(TaskError, "dependencies are unresolved"):
            self.db.complete(task, "codex", "Done", "Evidence")
        self.db.claim(dependency, "codex")
        with self.assertRaisesRegex(TaskError, "requires both"):
            self.db.complete(dependency, "codex", "Done", " ")
        self.db.complete(dependency, "codex", "Dependency completed", "test run 1")
        self.db.complete(task, "codex", "Dependent completed", "test run 2")
        self.assertEqual(self.db.get(task)["status"], "completed")
        event_types = [event["event_type"] for event in self.db.history(task)]
        self.assertIn("completed", event_types)
        with self.assertRaisesRegex(TaskError, "use complete"):
            self.db.update(task, "codex", status="completed")
        with self.assertRaisesRegex(TaskError, "cannot add dependencies"):
            self.db.add_dependency(task, dependency, "codex")

    def test_reassignment_and_review_decisions_are_audited(self):
        task = self.make_task()
        self.db.assign(task, "copilot", "codex")
        self.db.claim(task, "copilot")
        self.db.update(task, "copilot", status="review")
        self.db.review(task, "copilot", "request_changes", "Add an error-path assertion")
        events = self.db.history(task)
        self.assertIn("assignment_changed", [event["event_type"] for event in events])
        review_event = next(event for event in events if event["event_type"] == "review_decision")
        self.assertIn('"request_changes"', review_event["details_json"])
        with self.assertRaisesRegex(TaskError, "only codex may reassign"):
            self.db.assign(task, "big-pickle", "copilot")

    def test_schema_mismatch_is_rejected_without_database_changes(self):
        with sqlite3.connect(self.path) as connection:
            connection.execute("PRAGMA user_version = 999")
        before = self.path.read_bytes()
        with self.assertRaisesRegex(TaskError, "unsupported task database schema version 999"):
            self.db.list()
        self.assertEqual(self.path.read_bytes(), before)

    def test_cli_initializes_and_refuses_a_second_init(self):
        script = Path(__file__).with_name("task.py")
        isolated = Path(self.temp.name) / "cli.db"
        first = subprocess.run([sys.executable, str(script), "--db", str(isolated), "init"],
                               check=False, capture_output=True, text=True)
        self.assertEqual(first.returncode, 0, first.stderr)
        second = subprocess.run([sys.executable, str(script), "--db", str(isolated), "init"],
                                check=False, capture_output=True, text=True)
        self.assertNotEqual(second.returncode, 0)
        self.assertIn("refusing to overwrite", second.stderr)

    def test_cli_create_claim_filter_and_history_roundtrip(self):
        script = Path(__file__).with_name("task.py")
        isolated = Path(self.temp.name) / "roundtrip.db"

        def cli(*arguments):
            return subprocess.run([sys.executable, str(script), "--db", str(isolated), *arguments],
                                  check=False, capture_output=True, text=True)

        self.assertEqual(cli("init").returncode, 0)
        created = cli("create", "--title", "CLI task", "--description", "A detailed CLI fixture",
                      "--agent", "big-pickle", "--priority", "critical", "--actor", "codex")
        self.assertEqual(created.returncode, 0, created.stderr)
        task_id = created.stdout.strip()
        self.assertEqual(cli("claim", task_id, "--actor", "big-pickle").returncode, 0)
        listing = cli("list", "--agent", "big-pickle", "--status", "in_progress")
        self.assertEqual(listing.returncode, 0, listing.stderr)
        self.assertEqual(json.loads(listing.stdout)[0]["title"], "CLI task")
        history = cli("history", task_id)
        self.assertEqual(history.returncode, 0, history.stderr)
        self.assertEqual([event["event_type"] for event in json.loads(history.stdout)],
                         ["created", "status_changed"])

    def test_concurrent_writes_and_reopening_preserve_events(self):
        task = self.make_task("Concurrent", "big-pickle")

        def write_comment(number):
            self.db.comment(task, "big-pickle", f"Concurrent note {number}")

        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
            list(pool.map(write_comment, range(24)))
        reopened = TaskDatabase(self.path)
        comments = [row for row in reopened.history(task) if row["event_type"] == "comment"]
        self.assertEqual(len(comments), 24)
        self.assertEqual(len({row["id"] for row in comments}), 24)


if __name__ == "__main__":
    unittest.main()
