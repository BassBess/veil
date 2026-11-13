import json
import socket
import subprocess
import sys
import unittest
from unittest.mock import patch
from veil_engine.context_rules import suggestions
from veil_engine.detection_options import filter_candidates, validate_options, literal_matches
from veil_engine.detector import VeilDetector
from veil_engine.languages import ModelUnavailable


def candidate(text, value, category="PERSON", score=.85):
    start = text.index(value)
    return {"text": value, "entity_type": category, "start": start, "end": start + len(value), "confidence": score}


class TestDocumentRules(unittest.TestCase):
    def test_literal_rules_do_not_split_combining_marks_or_identifiers(self):
        self.assertEqual(list(literal_matches("عليّ", "علي")), [])
        self.assertEqual(list(literal_matches("project_John", "John")), [])
        self.assertEqual(len(list(literal_matches("🙂 هِبة سُويدان", "هِبة سُويدان"))), 1)

    def test_exclusions_are_exact_category_scoped_and_inspectable(self):
        text = "Orange works with Orange Labs."
        hits = [candidate(text, "Orange", "ORGANIZATION"), candidate(text, "Orange", "PERSON"), candidate(text, "Orange Labs", "ORGANIZATION")]
        accepted, suppressed = filter_candidates(text, hits, {"ignored_phrases": [{"phrase": "orange", "entity_type": "ORGANIZATION"}]})
        self.assertEqual(len(accepted), 2)
        self.assertEqual(suppressed[0]["reason"], "Ignored phrase")

    def test_context_respects_distance_and_sentence_boundaries(self):
        rules = {"required_context": [{"phrase": "passport", "entity_type": "ID_NUMBER"}]}
        for text, expected in [("passport: AB12345", True), ("passport. AB12345", False), ("passport\nAB12345", False), ("passport " + "x" * 90 + " AB12345", False), ("AB12345 (passport)", True)]:
            with self.subTest(text=text):
                accepted, _ = filter_candidates(text, [candidate(text, "AB12345", "ID_NUMBER")], rules)
                self.assertEqual(bool(accepted), expected)

    def test_context_alternatives_and_arabic(self):
        text = "🙂 رقم المريض: ١٢٣٤٥٦"
        options = {"required_context": [{"phrase": "passport", "entity_type": "ID_NUMBER"}, {"phrase": "رقم المريض", "entity_type": "ID_NUMBER"}]}
        accepted, suppressed = filter_candidates(text, [candidate(text, "١٢٣٤٥٦", "ID_NUMBER")], options)
        self.assertEqual(len(accepted), 1)
        self.assertFalse(suppressed)
        self.assertEqual(text[accepted[0]["start"]:accepted[0]["end"]], "١٢٣٤٥٦")

    def test_field_labels_are_suppressed_but_explicit_phrases_can_override(self):
        for language, label in [("fr", "Courriel"), ("es", "Correo"), ("ar", "الهاتف")]:
            text = label + ": example@example.com"
            hit = candidate(text, label, "LOCATION")
            accepted, suppressed = filter_candidates(text, [hit], {"language": language})
            self.assertFalse(accepted)
            self.assertEqual(suppressed[0]["reason"], "Field label, not an identity")
            accepted, _ = filter_candidates(text, [{**hit, "source": "custom phrase"}], {"language": language})
            self.assertEqual(len(accepted), 1)

    def test_low_scoring_suggestions_are_kept(self):
        text = "John met Jane"
        hits = [candidate(text, "John", score=.4), candidate(text, "Jane", score=.9)]
        accepted, suppressed = filter_candidates(text, hits, {})
        self.assertEqual(accepted, hits)
        self.assertFalse(suppressed)

    def test_validation(self):
        for rules in [[{"phrase": "x", "entity_type": "UNKNOWN"}], [{"phrase": "ع" * 251, "entity_type": "PERSON"}], [{"phrase": "x", "entity_type": "PERSON"}] * 201]:
            with self.assertRaises(ValueError):
                validate_options({"session_rules": rules}, VeilDetector.SUPPORTED_ENTITIES)

    def test_localized_labels_preserve_unicode(self):
        for language, text, expected in [
            ("fr", "🙂 Passeport: AB1234567\nCode postal: 75001.", {"AB1234567", "75001"}),
            ("es", "🙂 Pasaporte: AB1234567\nCódigo postal: 28001.", {"AB1234567", "28001"}),
            ("ar", "🙂 رقم جواز السفر: AB١٢٣٤٥٦٧\nالرمز البريدي: ١٢٣٤٥.", {"AB١٢٣٤٥٦٧", "١٢٣٤٥"}),
        ]:
            with self.subTest(language=language):
                hits = suggestions(text, {"ID_NUMBER", "POSTAL_CODE"}, [], language=language)
                self.assertEqual({hit["text"] for hit in hits}, expected)
                for hit in hits:
                    self.assertEqual(text[hit["start"]:hit["end"]], hit["text"])

    def test_localized_dates_and_plain_numbers(self):
        for language, text in [("fr", "12 février 2020"), ("es", "12 de febrero de 2020"), ("ar", "١٢ فبراير ٢٠٢٠")]:
            with self.subTest(language=language):
                self.assertEqual(suggestions(text, {"DATE_TIME"}, [], language=language)[0]["text"], text)
                self.assertEqual(suggestions("12345 items", {"POSTAL_CODE", "ID_NUMBER"}, [], language=language), [])

    def test_missing_model_never_downloads(self):
        with patch("spacy.util.is_package", return_value=False), patch.object(socket.socket, "connect", side_effect=AssertionError("No network")):
            with self.assertRaisesRegex(ModelUnavailable, "French model"):
                VeilDetector("fr")

    def test_unsupported_language_and_bridge_error(self):
        with self.assertRaisesRegex(ValueError, "Choose English"):
            VeilDetector("de")
        request = {"text": "Jean Dupont", "language": "de", "entities": ["PERSON"]}
        result = subprocess.run([sys.executable, "-m", "veil_engine.bridge"], input=json.dumps(request), text=True, capture_output=True)
        self.assertEqual(result.returncode, 0)
        self.assertIn("Choose English", json.loads(result.stdout)["error"]["message"])
        self.assertNotIn("Jean Dupont", result.stdout)


class TestInstalledLanguageModels(unittest.TestCase):
    def test_phone_detection_checks_all_regions_without_configuration(self):
        detector = VeilDetector("fr")
        text = "Téléphone: 06 12 34 56 78; +34 612 345 678; +961 71 234 567; +61 412 345 678"
        hits = detector.suggest(text, ["PHONE_NUMBER"])
        for number in ["06 12 34 56 78", "+34 612 345 678", "+961 71 234 567", "+61 412 345 678"]:
            self.assertTrue(any(hit["text"] == number for hit in hits), number)
        for hit in hits:
            self.assertEqual(text[hit["start"]:hit["end"]], hit["text"])

    def test_real_models_offline(self):
        examples = [
            ("en", "John Smith lives in New York. Email: john@example.com.", "John Smith", "New York"),
            ("fr", "Jean Dupont habite à Paris. Courriel : jean@example.com.", "Jean Dupont", "Paris"),
            ("es", "María García vive en Madrid. Correo: maria@example.com.", "María García", "Madrid"),
            ("ar", "محمد أحمد يعيش في بيروت. البريد الإلكتروني: mohammad@example.com.", "محمد أحمد", "بيروت"),
        ]
        for language, text, name, location in examples:
            with self.subTest(language=language), patch.object(socket.socket, "connect", side_effect=AssertionError("Detection must remain offline")):
                detector = VeilDetector(language)
                hits = detector.suggest("🙂 " + text, ["PERSON", "LOCATION", "EMAIL_ADDRESS"])
                values = {(hit["entity_type"], hit["text"]) for hit in hits}
                self.assertIn(("PERSON", name), values)
                self.assertIn(("LOCATION", location), values)
                self.assertTrue(any(hit["entity_type"] == "EMAIL_ADDRESS" for hit in hits))
                for hit in hits:
                    self.assertEqual(("🙂 " + text)[hit["start"]:hit["end"]], hit["text"])
                hits = detector.suggest(text, ["PERSON"], options={"ignored_phrases": [{"phrase": name, "entity_type": "PERSON"}]})
                self.assertNotIn(name, [hit["text"] for hit in hits])
                self.assertTrue(any(hit["reason"] == "Ignored phrase" for hit in detector.suppressed))

    def test_arabic_diacritics_and_digits(self):
        detector = VeilDetector("ar")
        text = "🙂 تواصل مع هِبة سُويدان. الهاتف: +٩٦١ ٧١ ١٢٣ ٤٥٦"
        hits = detector.suggest(text, ["PERSON", "PHONE_NUMBER"], options={"session_rules": [{"phrase": "هِبة سُويدان", "entity_type": "PERSON"}]})
        self.assertTrue(any(hit["text"] == "هِبة سُويدان" for hit in hits))
        self.assertTrue(any(hit["entity_type"] == "PHONE_NUMBER" and "٩٦١" in hit["text"] for hit in hits))
        for hit in hits:
            self.assertEqual(text[hit["start"]:hit["end"]], hit["text"])
