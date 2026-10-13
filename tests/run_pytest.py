import sys
from pathlib import Path

import pytest


class CollectionTracker:
    def __init__(self) -> None:
        self.has_tests = False
        self.runner_path = Path.cwd() / "tests/run_pytest.py"
        self.empty_initializers = {
            self.runner_path.with_name("__init__.py").resolve(),
            (self.runner_path.parents[1] / "stubs/__init__.py").resolve(),
        }

    def pytest_collectstart(self, collector: pytest.Collector) -> None:
        path = collector.path.resolve()
        if not path.is_file() or path == self.runner_path:
            return
        if path in self.empty_initializers and path.stat().st_size == 0:
            return
        self.has_tests = True

    def pytest_itemcollected(self) -> None:
        self.has_tests = True

    def pytest_deselected(self, items: list[pytest.Item]) -> None:
        if items:
            self.has_tests = True


def main() -> int:
    tracker = CollectionTracker()
    exit_code = pytest.main(sys.argv[1:], plugins=[tracker])
    if exit_code == pytest.ExitCode.NO_TESTS_COLLECTED and not tracker.has_tests:
        print("No user test files or items collected; skipping test execution.")
        return 0
    return int(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
