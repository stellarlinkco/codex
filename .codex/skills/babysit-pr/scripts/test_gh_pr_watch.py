import importlib.util
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("gh_pr_watch.py")
SPEC = importlib.util.spec_from_file_location("gh_pr_watch", MODULE_PATH)
gh_pr_watch = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(gh_pr_watch)


class GhPrWatchTests(unittest.TestCase):
    def test_classify_pending_checks_marks_long_running_and_stalled(self):
        checks = [
            {
                "name": "Tests — ubuntu-24.04 - x86_64-unknown-linux-gnu",
                "workflow": "rust-ci",
                "state": "IN_PROGRESS",
                "bucket": "pending",
                "startedAt": "2026-03-25T01:00:00Z",
                "link": "https://example.test/rust-ci",
            },
            {
                "name": "Local Bazel build on ubuntu-24.04 for x86_64-unknown-linux-gnu",
                "workflow": "Bazel (experimental)",
                "state": "IN_PROGRESS",
                "bucket": "pending",
                "startedAt": "2026-03-25T01:10:00Z",
                "link": "https://example.test/bazel",
            },
        ]

        real_datetime = gh_pr_watch.datetime

        class FixedDateTime(real_datetime):
            @classmethod
            def now(cls, tz=None):
                return cls.fromisoformat("2026-03-25T01:31:00+00:00")

        gh_pr_watch.datetime = FixedDateTime
        try:
            result = gh_pr_watch.classify_pending_checks(checks)
        finally:
            gh_pr_watch.datetime = real_datetime

        self.assertEqual(
            result["stalled_checks"],
            [
                {
                    "name": "Tests — ubuntu-24.04 - x86_64-unknown-linux-gnu",
                    "workflow": "rust-ci",
                    "started_at": "2026-03-25T01:00:00Z",
                    "age_seconds": 1860,
                    "warn_seconds": 900,
                    "stuck_seconds": 1500,
                    "details_url": "https://example.test/rust-ci",
                }
            ],
        )
        self.assertEqual(
            result["long_running_checks"],
            [
                {
                    "name": "Tests — ubuntu-24.04 - x86_64-unknown-linux-gnu",
                    "workflow": "rust-ci",
                    "started_at": "2026-03-25T01:00:00Z",
                    "age_seconds": 1860,
                    "warn_seconds": 900,
                    "stuck_seconds": 1500,
                    "details_url": "https://example.test/rust-ci",
                }
            ],
        )

    def test_recommend_actions_prefers_stall_diagnosis_over_idle(self):
        actions = gh_pr_watch.recommend_actions(
            pr={
                "closed": False,
                "merged": False,
                "mergeable": "MERGEABLE",
                "merge_state_status": "UNSTABLE",
                "review_decision": "",
            },
            checks_summary={
                "all_terminal": False,
                "failed_count": 0,
                "pending_count": 1,
                "passed_count": 10,
            },
            failed_runs=[],
            new_review_items=[],
            retries_used=0,
            max_retries=3,
            stalled_checks=[
                {
                    "workflow": "rust-ci",
                    "name": "Tests — ubuntu-24.04 - x86_64-unknown-linux-gnu",
                    "age_seconds": 1860,
                }
            ],
        )

        self.assertEqual(actions, ["diagnose_ci_stall"])


if __name__ == "__main__":
    unittest.main()
