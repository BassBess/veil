import re
import random
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from veil_engine.document_parser import parse_document, DocumentError, MAX_TEXT_BYTES, MAX_FILE_BYTES
from veil_engine.detector import VeilDetector


class FakeAnalyzer:
    def __init__(self):
        self.window_sizes = []
    def get_supported_entities(self, language):
        return ["EMAIL_ADDRESS"]
    def analyze(self, text, language, entities):
        self.window_sizes.append(len(text))
        return [SimpleNamespace(entity_type="EMAIL_ADDRESS", start=m.start(), end=m.end(), score=.9)
                for m in re.finditer(r"john@example\.com", text)]


class TestLargeDocuments(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.path = Path(self.directory.name) / "text.txt"

    def test_utf8_text_boundary_is_bytes_not_characters(self):
        self.path.write_bytes(("é" * (MAX_TEXT_BYTES // 2)).encode("utf-8"))
        self.assertEqual(len(parse_document(self.path)["text"].encode("utf-8")), MAX_TEXT_BYTES)
        with self.path.open("ab") as stream:
            stream.write(b"x")
        with self.assertRaises(DocumentError) as error:
            parse_document(self.path)
        self.assertEqual(error.exception.code, "text_too_large")

    def test_python_enforces_requested_and_absolute_file_limits(self):
        self.path.write_bytes(b"abc")
        with self.assertRaises(DocumentError) as error:
            parse_document(self.path, 2)
        self.assertEqual(error.exception.code, "file_too_large")
        for limit in [0, MAX_FILE_BYTES + 1, True, "100"]:
            with self.subTest(limit=limit), self.assertRaises(DocumentError) as error:
                parse_document(self.path, limit)
            self.assertEqual(error.exception.code, "invalid_limit")

    def test_text_above_old_limit_is_not_truncated(self):
        source = "Ordinary text. " * 10_000 + "john@example.com"
        self.path.write_text(source, encoding="utf-8")
        self.assertEqual(parse_document(self.path)["text"], source)

    def test_chunk_overlap_deduplicates_and_keeps_global_unicode_positions(self):
        detector = VeilDetector.__new__(VeilDetector)
        detector.analyzer = FakeAnalyzer()
        source = "🙂" * 19_000 + "john@example.com " + "a " * 15_000 + "john@example.com"
        with patch("socket.socket.connect", side_effect=AssertionError("Unexpected network access")):
            results = detector.suggest(source, ["EMAIL_ADDRESS"])
        self.assertEqual(len(results), 2)
        self.assertEqual(results[0]["start"], 19_000)
        self.assertEqual(results[1]["end"], len(source))
        self.assertTrue(all(source[item["start"]:item["end"]] == item["text"] for item in results))
        self.assertTrue(all(size <= 20_000 for size in detector.analyzer.window_sizes))
        self.assertGreater(len(detector.analyzer.window_sizes), 1)

    def test_custom_phrase_crossing_chunk_boundary_is_whole(self):
        detector = VeilDetector.__new__(VeilDetector)
        detector.analyzer = FakeAnalyzer()
        source = "x " * 9_998 + "Émile Dupont" + " y" * 1_000
        results = detector.suggest(source, ["PERSON"], [{"phrase":"Émile Dupont", "entity_type":"PERSON"}])
        self.assertEqual(len(results), 1)
        self.assertEqual(results[0]["text"], "Émile Dupont")
        self.assertEqual(source[results[0]["start"]:results[0]["end"]], "Émile Dupont")

    def test_oversized_detection_is_rejected_before_nlp(self):
        detector = VeilDetector.__new__(VeilDetector)
        detector.analyzer = FakeAnalyzer()
        with self.assertRaises(ValueError):
            detector.suggest("a" * (MAX_TEXT_BYTES + 1), ["EMAIL_ADDRESS"])
        self.assertEqual(detector.analyzer.window_sizes, [])

    def test_indexed_overlap_resolution_matches_greedy_reference(self):
        rng = random.Random(42)
        detector = VeilDetector.__new__(VeilDetector)
        for _ in range(20):
            candidates = []
            for index in range(80):
                start = rng.randrange(100)
                end = start + rng.randrange(1, 20)
                candidates.append({"start":start, "end":end, "text":"x"*(end-start),
                                   "entity_type":rng.choice(["PERSON","STREET_ADDRESS","EMAIL_ADDRESS"]),
                                   "confidence":rng.choice([.4,.6,.9])})
            detector.detect = lambda *args: candidates
            expected = []
            for item in sorted(candidates, key=lambda x:(x["entity_type"] in {"STREET_ADDRESS","POSTAL_CODE","INDIRECT_IDENTIFIER"},x["confidence"],x["end"]-x["start"]), reverse=True):
                if not any(item["start"] < old["end"] and item["end"] > old["start"] for old in expected):
                    expected.append(item)
            self.assertEqual(detector.suggest("x"*120), sorted(expected,key=lambda x:x["start"]))
