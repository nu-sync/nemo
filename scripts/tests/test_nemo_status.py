import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from nemo_status import lifecycle_status


class LifecycleStatusTests(unittest.TestCase):
    def test_recording_remains_observable_when_the_shortcut_is_blocked(self):
        requests = []

        def query(command):
            requests.append(command)
            return json.dumps({"activity": "recording", "blockers": ["accessibility"]})

        self.assertEqual(lifecycle_status(query), "Recording")
        self.assertEqual(requests, ["readiness"])

    def test_success_unload_and_failure_preserve_the_observed_outcome(self):
        for text, expected in [
            ("Ready — Right Option · Transcript delivered", "Ready"),
            ("Ready — Right Option · Model unloaded", "Ready · model unloaded"),
            ("Right Option blocked: enable Accessibility · No audio detected", "No audio detected"),
        ]:
            def query(command):
                return json.dumps({"activity": "idle"}) if command == "readiness" else text

            with self.subTest(text=text):
                self.assertEqual(lifecycle_status(query), expected)

    def test_older_daemons_keep_their_existing_status_protocol(self):
        replies = {"readiness": "unknown command", "status": "Ready · model unloaded"}
        self.assertEqual(lifecycle_status(replies.__getitem__), "Ready · model unloaded")


if __name__ == "__main__":
    unittest.main()
