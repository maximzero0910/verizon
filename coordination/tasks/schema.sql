PRAGMA foreign_keys = ON;

CREATE TABLE tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL CHECK (length(trim(title)) > 0),
    description TEXT NOT NULL CHECK (length(trim(description)) > 0),
    assigned_agent TEXT NOT NULL CHECK (assigned_agent IN ('codex', 'big-pickle', 'copilot')),
    created_by TEXT NOT NULL CHECK (created_by IN ('codex', 'big-pickle', 'copilot', 'user')),
    priority TEXT NOT NULL CHECK (priority IN ('critical', 'high', 'medium', 'low')),
    status TEXT NOT NULL CHECK (status IN ('open', 'in_progress', 'blocked', 'review', 'completed', 'cancelled')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deadline TEXT,
    completion_summary TEXT,
    verification_evidence TEXT,
    CHECK ((status != 'completed') OR
           (completion_summary IS NOT NULL AND length(trim(completion_summary)) > 0 AND
            verification_evidence IS NOT NULL AND length(trim(verification_evidence)) > 0))
);

CREATE TABLE task_dependencies (
    task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    depends_on INTEGER NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    PRIMARY KEY (task_id, depends_on),
    CHECK (task_id != depends_on)
);

CREATE TABLE task_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    event_type TEXT NOT NULL CHECK (event_type IN (
        'created', 'assignment_changed', 'status_changed', 'priority_changed',
        'blocker', 'comment', 'completed', 'review_decision', 'dependency_added'
    )),
    actor TEXT NOT NULL CHECK (actor IN ('codex', 'big-pickle', 'copilot', 'user')),
    timestamp TEXT NOT NULL,
    message TEXT NOT NULL,
    details_json TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX tasks_status_idx ON tasks(status);
CREATE INDEX tasks_agent_idx ON tasks(assigned_agent);
CREATE INDEX task_events_task_idx ON task_events(task_id, id);
CREATE INDEX task_dependencies_parent_idx ON task_dependencies(depends_on);
