import unittest
from veil_engine.detector import VeilDetector


class TestVeilDetector(unittest.TestCase):

    @classmethod
    def setUpClass(cls):
        cls.detector = VeilDetector()

    def test_email_detection(self):
        results = self.detector.suggest(
            "Contact john@example.com"
        )

        self.assertTrue(any(
            item["entity_type"] == "EMAIL_ADDRESS"
            and item["text"] == "john@example.com"
            for item in results
        ))

    def test_no_overlapping_suggestions(self):
        text = "Email: john.smith@example.com"
        results = self.detector.suggest(text)

        for first, second in zip(results, results[1:]):
            self.assertLessEqual(
                first["end"],
                second["start"]
            )

    def test_invalid_input(self):
        with self.assertRaises(TypeError):
            self.detector.detect(123)


if __name__ == "__main__":
    unittest.main()
