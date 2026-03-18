import unittest
from unittest.mock import patch

import configure_review_automation as setup


class ConfigureReviewAutomationTests(unittest.TestCase):
    def test_normalize_app_ids_dedupes_csv_inputs(self):
        self.assertEqual(setup._normalize_app_ids(["101,102", "102", "103"]), [101, 102, 103])

    def test_discover_claude_app_ids_reads_marker_bearing_comments_and_reviews(self):
        comments = [
            {
                "body": "<!-- claude-review-findings -->\n{}",
                "performed_via_github_app": {"id": 111},
            },
            {
                "body": "not a claude payload",
                "performed_via_github_app": {"id": 999},
            },
        ]
        reviews = [
            {
                "body": "<!-- claude-review-findings -->\n{}",
                "performed_via_github_app": {"id": 222},
            }
        ]

        with patch.object(setup, "_gh_api_json", side_effect=[comments, reviews]):
            self.assertEqual(setup.discover_claude_app_ids("owner/repo", 42), [111, 222])


if __name__ == "__main__":
    unittest.main()
