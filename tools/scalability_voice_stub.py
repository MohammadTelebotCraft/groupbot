"""Synthetic process-IPC fixture. Does not open media or contact a recognizer."""
import json
import sys
import time

for line in sys.stdin:
    request = json.loads(line)
    assert request["input"] == "offline-fixture"
    time.sleep(0.1)
    print(json.dumps({"transcripts": [{"text": "fixture", "confidence": 1.0}], "usable_windows": 1}), flush=True)
