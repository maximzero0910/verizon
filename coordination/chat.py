#!/usr/bin/env python3
"""Stable coordination entry point for the existing shared agent chat."""

import runpy
from pathlib import Path


if __name__ == "__main__":
    helper = Path(__file__).resolve().parents[1] / "tools" / "shared_agent_chat.py"
    runpy.run_path(str(helper), run_name="__main__")
