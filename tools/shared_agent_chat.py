#!/usr/bin/env python3
"""Read and safely append to the repository's shared agent chat."""

import argparse
import fcntl
import json
import os
import tempfile
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CHAT = ROOT / "coordination" / "CHAT.json"
SENDERS = ("codex", "copilot", "big-pickle")


@contextmanager
def chat_lock():
    CHAT.parent.mkdir(parents=True, exist_ok=True)
    directory_fd = os.open(CHAT.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        fcntl.flock(directory_fd, fcntl.LOCK_EX)
        yield
    finally:
        fcntl.flock(directory_fd, fcntl.LOCK_UN)
        os.close(directory_fd)


def load_chat():
    if not CHAT.exists():
        return {"messages": []}
    with CHAT.open(encoding="utf-8") as source:
        data = json.load(source)
    if not isinstance(data, dict) or set(data) != {"messages"}:
        raise ValueError("chat must be an object containing only 'messages'")
    messages = data["messages"]
    if not isinstance(messages, list):
        raise ValueError("'messages' must be an array")
    for expected_id, message in enumerate(messages, 1):
        if not isinstance(message, dict) or message.get("id") != expected_id:
            raise ValueError("message IDs must be unique and sequential starting at 1")
        if set(message) != {"id", "sender", "timestamp", "message", "reply_to"}:
            raise ValueError(f"message {expected_id} has an invalid shape")
    return data


def save_chat(data):
    encoded = json.dumps(data, ensure_ascii=False, indent=2) + "\n"
    fd, temporary = tempfile.mkstemp(prefix=".CHAT.json.", dir=CHAT.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as target:
            target.write(encoded)
            target.flush()
            os.fsync(target.fileno())
        os.replace(temporary, CHAT)
        directory_fd = os.open(CHAT.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def read_chat(_args):
    with chat_lock():
        data = load_chat()
    print(json.dumps(data, ensure_ascii=False, indent=2))


def append_message(args):
    with chat_lock():
        data = load_chat()
        messages = data["messages"]
        if args.reply_to is not None and not 1 <= args.reply_to <= len(messages):
            raise ValueError(f"reply_to must reference an existing message ID (1..{len(messages)})")
        messages.append({
            "id": len(messages) + 1,
            "sender": args.sender,
            "timestamp": datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z"),
            "message": args.message,
            "reply_to": args.reply_to,
        })
        save_chat(data)
    print(messages[-1]["id"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("read", help="print the complete conversation").set_defaults(func=read_chat)
    append = commands.add_parser("append", help="append one message")
    append.add_argument("--sender", choices=SENDERS, required=True)
    append.add_argument("--message", required=True)
    append.add_argument("--reply-to", type=int)
    append.set_defaults(func=append_message)
    args = parser.parse_args()
    try:
        args.func(args)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        parser.exit(1, f"shared-agent-chat: {error}\n")


if __name__ == "__main__":
    main()