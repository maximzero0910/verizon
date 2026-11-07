# J2H Task Board

This is a local, standalone Python and SQLite task manager for the J2H agent team. It has no server or network component. `CHAT.json` remains the communication history; `tasks.db` is authoritative for task assignment, status, dependencies, and audit events. Use `python3 coordination/chat.py read` and `python3 coordination/chat.py append ...` for chat; that wrapper delegates to the existing `tools/shared_agent_chat.py` implementation and does not merge chat into SQLite.

## Initialize

Run once from the repository root:

```sh
python3 coordination/tasks/task.py init
```

Initialization creates `coordination/tasks/tasks.db` and stores schema version 1 in `PRAGMA user_version`. It refuses to initialize over any existing path. Back up the database before any manual recovery or migration. Each connection enables foreign keys and a 5 second busy timeout; writes use SQLite `BEGIN IMMEDIATE` transactions.

Use `--db /path/to/another.db` before a command to work with an isolated database. The schema is in `schema.sql`; future changes must add an explicit schema migration and increment `SCHEMA_VERSION` in `database.py`.

## Commands

```sh
python3 coordination/tasks/task.py list
python3 coordination/tasks/task.py list --agent codex
python3 coordination/tasks/task.py list --status blocked
python3 coordination/tasks/task.py show 1
python3 coordination/tasks/task.py history 1
python3 coordination/tasks/task.py board

python3 coordination/tasks/task.py create \
  --title "Investigate Mixin inner-class generation" \
  --description "Research Knot and Mixin generated class loading." \
  --agent big-pickle --priority critical --actor codex

python3 coordination/tasks/task.py claim 1 --actor big-pickle
python3 coordination/tasks/task.py update 1 --status blocked \
  --blocker "Waiting for pinned loader source evidence" --actor big-pickle
python3 coordination/tasks/task.py assign 1 --agent codex --actor codex
python3 coordination/tasks/task.py depend 2 --on 1 --actor codex
python3 coordination/tasks/task.py comment 1 --message "Found a classloader limitation." --actor big-pickle
python3 coordination/tasks/task.py review 2 --decision request_changes \
  --message "Add an append-failure regression." --actor copilot
python3 coordination/tasks/task.py complete 1 \
  --result "Investigation completed." \
  --evidence "Findings recorded in CHAT.json message 27." --actor codex
```

Mutating commands require `--actor`; the CLI does not infer an agent identity from the operating system. Actor checks enforce task ownership in normal CLI use but are not cryptographic authentication: local users with filesystem access can edit the SQLite database directly.

## Team and workflow

- **codex** — Project Lead, Lead Architect, Implementation Engineer.
- **big-pickle** — Independent Research Specialist.
- **copilot** — Independent Reviewer and Test Engineer.
- **user** — Overall project owner; can create tasks and comment, but cannot claim or complete another agent's task.

1. Check `list --agent <your-agent>` and `CHAT.json` for assignments and new messages.
2. Claim an assigned `open` task before starting: `claim ID --actor <your-agent>`.
3. Only the assignee may claim or update a task. Codex may update or explicitly reassign any task with `assign`.
4. Record meaningful progress with `comment`. When blocked, set `--status blocked` and give `--blocker`; report significant findings in `CHAT.json` as well.
5. Move work requiring independent verification to `review`. Copilot records `approve` or `request_changes` with `review`; the decision is retained in event history.
6. Codex marks verified work complete with both `--result` and `--evidence`. A dependency must be `completed` before a dependent task can be completed.
7. Task and history rows are not deleted by the CLI. Every assignment, status, priority, blocker, comment, dependency, review, and completion action is recorded in `task_events`.

Completion and review evidence must point to real test output, source evidence, or a durable result in the repository/chat. Do not claim success based only on JAR discovery or compilation.

## Tests

```sh
python3 -m unittest discover -s coordination/tasks -p 'test_*.py' -v
```

The tests use temporary databases and do not change the shared backlog.
