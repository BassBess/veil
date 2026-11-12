import io
import unittest
from unittest.mock import patch
import pymupdf
from veil_engine.pdf_export import export_pdf
from veil_engine.context_rules import suggestions
from veil_engine.detector import VeilDetector


class TestAddedDetection(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.detector = VeilDetector()

    def test_address_and_postcode_are_reviewable_selected_categories(self):
        text = "Address: 21 Main Street\nZIP: 02110."
        hits = self.detector.suggest(text, ["STREET_ADDRESS", "POSTAL_CODE"])
        self.assertTrue(any(hit["entity_type"] == "STREET_ADDRESS" and "21 Main Street" in hit["text"] for hit in hits))
        self.assertTrue(any(hit["entity_type"] == "POSTAL_CODE" and hit["text"] == "02110" for hit in hits))
        self.assertEqual(self.detector.suggest(text, []), [])

    def test_labeled_identifier_is_detected(self):
        hits = self.detector.suggest("Passport: AB1234567", ["ID_NUMBER"])
        self.assertTrue(any(hit["text"] == "AB1234567" for hit in hits))

    def test_custom_phrase_is_literal_category_filtered_and_unicode_safe(self):
        text = "\U0001f600 The rare local role and C++ specialist."
        rules = [{"phrase": "C++ specialist", "entity_type": "INDIRECT_IDENTIFIER"}]
        hits = self.detector.suggest(text, ["INDIRECT_IDENTIFIER"], rules)
        hit = next(hit for hit in hits if hit["text"] == "C++ specialist")
        self.assertEqual(text[hit["start"]:hit["end"]], "C++ specialist")
        self.assertFalse(any(item["entity_type"] == "INDIRECT_IDENTIFIER"
                             for item in self.detector.suggest(text, ["PERSON"], rules)))

    def test_indirect_description_suggested_without_claiming_uniqueness(self):
        hits = self.detector.suggest("The only baker in Lebanon called today.", ["INDIRECT_IDENTIFIER"])
        self.assertTrue(any("baker" in hit["text"].lower() for hit in hits))

    def test_plain_numbers_are_not_postcodes_without_context(self):
        self.assertEqual(suggestions("There are 12345 items.", {"POSTAL_CODE"}, []), [])


class TestPdfExport(unittest.TestCase):
    def test_processed_pdf_contains_output_and_no_original(self):
        data = export_pdf("PERSON_001 lives here.\nEmail: EMAIL_001")
        self.assertTrue(data.startswith(b"%PDF"))
        with pymupdf.open(stream=data, filetype="pdf") as pdf:
            text = "".join(page.get_text() for page in pdf)
        self.assertIn("PERSON_001", text)
        self.assertIn("EMAIL_001", text)
        self.assertNotIn("John Smith", text)

    def test_long_document_has_multiple_pages_and_tail(self):
        data = export_pdf("\n".join("PERSON_001 reviewed line " + str(index) for index in range(150)))
        with pymupdf.open(stream=data, filetype="pdf") as pdf:
            self.assertGreater(len(pdf), 1)
            self.assertIn("149", pdf[-1].get_text())

    def test_markup_is_escaped_and_export_does_not_fetch_resources(self):
        with patch("socket.socket", side_effect=AssertionError("Network is forbidden")):
            data = export_pdf('<img src="https://invalid.test/private"> PERSON_001')
        with pymupdf.open(stream=data, filetype="pdf") as pdf:
            self.assertIn("PERSON_001", pdf[0].get_text())
            self.assertIn("<img", pdf[0].get_text())

    def test_empty_text_is_rejected(self):
        with self.assertRaises(ValueError):
            export_pdf(" ")


class TestOfflineExtendedDetection(unittest.TestCase):
    def test_extended_detection_never_opens_network_connections(self):
        detector = VeilDetector()
        text = "Address: 21 Main Street\nZIP: 02110. Visit https://example.com or 192.168.1.1."
        with patch("socket.socket", side_effect=AssertionError("Network is forbidden")):
            hits = detector.suggest(text, ["STREET_ADDRESS", "POSTAL_CODE", "URL", "IP_ADDRESS"])
        self.assertTrue(any(hit["entity_type"] == "URL" for hit in hits))
        self.assertTrue(any(hit["entity_type"] == "IP_ADDRESS" for hit in hits))
