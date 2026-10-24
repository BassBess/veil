from pprint import pprint
from veil_engine.detector import VeilDetector

detector = VeilDetector()

text = (
    "John Smith lives in New York. "
    "His email is john.smith@example.com "
    "and his phone number is 212-555-1234."
)

print("\n=== ORIGINAL TEXT ===")
print(text)

print("\n=== DETECTED INFORMATION ===")
print("\n=== ALL DETECTIONS ===")
pprint(detector.detect(text))

print("\n=== SUGGESTED DETECTIONS ===")
pprint(detector.suggest(text))
