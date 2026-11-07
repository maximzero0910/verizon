import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
CHAT_HELPER = REPO_ROOT / "tools" / "shared_agent_chat.py"


class SharedAgentChatConcurrencyTests(unittest.TestCase):
    def test_simultaneous_process_appends_preserve_every_message(self):
        child = r"""
import argparse
import sys
import time
from pathlib import Path

sys.path.insert(0, sys.argv[1])
import shared_agent_chat as chat

chat.CHAT = Path(sys.argv[2])
save_chat = chat.save_chat

def slow_save(data):
    time.sleep(0.4)
    save_chat(data)

chat.save_chat = slow_save
chat.append_message(argparse.Namespace(
    sender="copilot", message=sys.argv[3], reply_to=None
))
"""
        with tempfile.TemporaryDirectory() as temporary:
            chat_file = Path(temporary) / "coordination" / "CHAT.json"
            chat_file.parent.mkdir()
            chat_file.write_text('{"messages": []}\n', encoding="utf-8")
            messages = [f"writer-{index}" for index in range(10)]
            processes = [
                subprocess.Popen(
                    [sys.executable, "-c", child, str(CHAT_HELPER.parent),
                     str(chat_file), message],
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                )
                for message in messages
            ]
            for process in processes:
                stdout, stderr = process.communicate(timeout=20)
                self.assertEqual(process.returncode, 0, f"{stdout}\n{stderr}")

            chat = json.loads(chat_file.read_text(encoding="utf-8"))
            entries = chat["messages"]
            self.assertEqual([entry["id"] for entry in entries], list(range(1, 11)))
            self.assertEqual({entry["message"] for entry in entries}, set(messages))
            self.assertTrue(all(entry["sender"] == "copilot" for entry in entries))


if __name__ == "__main__":
    unittest.main()