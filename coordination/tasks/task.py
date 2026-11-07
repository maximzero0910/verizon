#!/usr/bin/env python3
"""Manage the local J2H multi-agent task board."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from database import AGENTS, ACTORS, PRIORITIES, STATUSES, TaskDatabase, TaskError


DEFAULT_DB = Path(__file__).with_name("tasks.db")


def emit(value):
    print(json.dumps(value, ensure_ascii=False, indent=2))


def make_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", type=Path, default=DEFAULT_DB, help=f"database path (default: {DEFAULT_DB})")
    commands = parser.add_subparsers(dest="command", required=True)

    init = commands.add_parser("init", help="create a new task database (refuses to overwrite)")
    init.set_defaults(run=lambda args, db: (db.initialize(), print(f"Initialized {db.path}")))

    listing = commands.add_parser("list", help="list tasks")
    listing.add_argument("--agent", choices=AGENTS)
    listing.add_argument("--status", choices=STATUSES)
    listing.set_defaults(run=lambda args, db: emit(db.list(agent=args.agent, status=args.status)))

    show = commands.add_parser("show", help="show one task")
    show.add_argument("task_id", type=int)
    show.set_defaults(run=lambda args, db: emit(db.get(args.task_id)))

    create = commands.add_parser("create", help="create and assign a task")
    create.add_argument("--title", required=True)
    create.add_argument("--description", required=True)
    create.add_argument("--agent", choices=AGENTS, required=True)
    create.add_argument("--priority", choices=PRIORITIES, default="medium")
    create.add_argument("--deadline")
    create.add_argument("--depends-on", type=int, action="append", default=[])
    create.add_argument("--actor", choices=ACTORS, required=True)
    create.set_defaults(run=lambda args, db: print(db.create(
        title=args.title, description=args.description, agent=args.agent, actor=args.actor,
        priority=args.priority, deadline=args.deadline, dependencies=args.depends_on)))

    claim = commands.add_parser("claim", help="claim an assigned open task")
    claim.add_argument("task_id", type=int)
    claim.add_argument("--actor", choices=ACTORS, required=True)
    claim.set_defaults(run=lambda args, db: (db.claim(args.task_id, args.actor), print("Task claimed.")))

    update = commands.add_parser("update", help="change status or priority; use --blocker with blocked status")
    update.add_argument("task_id", type=int)
    update.add_argument("--status", choices=STATUSES)
    update.add_argument("--priority", choices=PRIORITIES)
    update.add_argument("--blocker")
    update.add_argument("--actor", choices=ACTORS, required=True)
    update.set_defaults(run=lambda args, db: (db.update(args.task_id, args.actor, status=args.status,
                                                       priority=args.priority, blocker=args.blocker),
                                             print("Task updated.")))

    assign = commands.add_parser("assign", help="reassign a task (Codex only)")
    assign.add_argument("task_id", type=int)
    assign.add_argument("--agent", choices=AGENTS, required=True)
    assign.add_argument("--actor", choices=ACTORS, required=True)
    assign.set_defaults(run=lambda args, db: (db.assign(args.task_id, args.agent, args.actor),
                                               print("Task assigned.")))

    dependency = commands.add_parser("depend", help="add a dependency to a task")
    dependency.add_argument("task_id", type=int)
    dependency.add_argument("--on", type=int, required=True, dest="dependency_id")
    dependency.add_argument("--actor", choices=ACTORS, required=True)
    dependency.set_defaults(run=lambda args, db: (db.add_dependency(args.task_id, args.dependency_id, args.actor),
                                                  print("Dependency added.")))

    comment = commands.add_parser("comment", help="add a task comment")
    comment.add_argument("task_id", type=int)
    comment.add_argument("--message", required=True)
    comment.add_argument("--actor", choices=ACTORS, required=True)
    comment.set_defaults(run=lambda args, db: (db.comment(args.task_id, args.actor, args.message),
                                                print("Comment recorded.")))

    review = commands.add_parser("review", help="record Copilot's independent review decision")
    review.add_argument("task_id", type=int)
    review.add_argument("--decision", choices=("approve", "request_changes"), required=True)
    review.add_argument("--message", required=True)
    review.add_argument("--actor", choices=ACTORS, required=True)
    review.set_defaults(run=lambda args, db: (db.review(args.task_id, args.actor, args.decision, args.message),
                                              print("Review decision recorded.")))

    complete = commands.add_parser("complete", help="complete a verified task (Codex only)")
    complete.add_argument("task_id", type=int)
    complete.add_argument("--result", required=True)
    complete.add_argument("--evidence", required=True)
    complete.add_argument("--actor", choices=ACTORS, required=True)
    complete.set_defaults(run=lambda args, db: (db.complete(args.task_id, args.actor, args.result, args.evidence),
                                                 print("Task completed.")))

    history = commands.add_parser("history", help="show immutable task event history")
    history.add_argument("task_id", type=int)
    history.set_defaults(run=lambda args, db: emit(db.history(args.task_id)))

    board = commands.add_parser("board", help="show task counts by status")
    def show_board(_args, db):
        rows = db.list()
        counts = {status: 0 for status in STATUSES}
        for row in rows:
            counts[row["status"]] += 1
        emit({"total": len(rows), "by_status": counts, "tasks": rows})
    board.set_defaults(run=show_board)
    return parser


def main(argv=None):
    parser = make_parser()
    args = parser.parse_args(argv)
    database = TaskDatabase(args.db)
    try:
        args.run(args, database)
    except (TaskError, OSError, ValueError) as error:
        parser.exit(1, f"j2h-tasks: {error}\n")


if __name__ == "__main__":
    main()
