import unittest

from veil_engine.detector import VeilDetector


class TestVeilDetector(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.detector = VeilDetector()

    def test_email_detection(self):
        results = self.detector.suggest("Contact john@example.com")

        self.assertTrue(
            any(
                item["entity_type"] == "EMAIL_ADDRESS"
                and item["text"] == "john@example.com"
                for item in results
            )
        )

    def test_no_overlapping_suggestions(self):
        text = "Email: john.smith@example.com"
        results = self.detector.suggest(text)

        for first, second in zip(results, results[1:]):
            self.assertLessEqual(
                first["end"],
                second["start"],
            )

    def test_invalid_input(self):
        with self.assertRaises(TypeError):
            self.detector.detect(123)

    def test_selected_entity_categories(self):
        text = (
            "John Smith lives in New York. "
            "Contact john@example.com"
        )

        results = self.detector.suggest(
            text,
            ["EMAIL_ADDRESS"],
        )

        self.assertTrue(
            any(
                item["entity_type"] == "EMAIL_ADDRESS"
                for item in results
            )
        )

        self.assertFalse(
            any(
                item["entity_type"] == "PERSON"
                for item in results
            )
        )

        self.assertFalse(
            any(
                item["entity_type"] == "LOCATION"
                for item in results
            )
        )

    def test_empty_entity_selection_returns_no_results(self):
        text = "John Smith lives in New York."

        results = self.detector.suggest(
            text,
            [],
        )

        self.assertEqual(results, [])


if __name__ == "__main__":
    unittest.main()