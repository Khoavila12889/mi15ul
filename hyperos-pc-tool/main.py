"""Entrypoint for HyperOS PC Optimizer (GUI by default, --cli for terminal)."""

from __future__ import annotations

import os
import sys

BASE_DIR = os.path.dirname(os.path.abspath(__file__))
if BASE_DIR not in sys.path:
    sys.path.insert(0, BASE_DIR)


def main() -> int:
    if len(sys.argv) > 1 and sys.argv[1] == "--cli":
        from cli import run_cli
        return run_cli(sys.argv[2:], BASE_DIR)

    from gui import launch_gui
    launch_gui(BASE_DIR)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
