from bisect import bisect_left
from veil_engine.languages import load_language, arabic_entities
from veil_engine.detection_options import validate_options, filter_candidates
from veil_engine.context_rules import suggestions
from tldextract import TLDExtract
from presidio_analyzer.predefined_recognizers import EmailRecognizer


class OfflineEmailRecognizer(EmailRecognizer):
    # Use the packaged public-suffix snapshot: never fetch updates or write a cache.
    _extract = TLDExtract(suffix_list_urls=(), cache_dir=None)

    def validate_result(self, pattern_text):
        return self._extract(pattern_text).fqdn != ""


class VeilDetector:
    SUPPORTED_ENTITIES = {
        "PERSON",
        "EMAIL_ADDRESS",
        "PHONE_NUMBER",
        "LOCATION",
        "ORGANIZATION",
        "STREET_ADDRESS", "POSTAL_CODE", "DATE_TIME", "CREDIT_CARD", "IBAN_CODE",
        "IP_ADDRESS", "URL", "ID_NUMBER", "INDIRECT_IDENTIFIER",
    }

    language = "en"
    arabic = None

    def __init__(self, language="en"):
        self.language = language
        self.analyzer, self.arabic = load_language(language, OfflineEmailRecognizer)

    def detect(
        self,
        text: str,
        entities: list[str] | None = None,
        rules: list[dict] | None = None,
    ) -> list[dict]:

        if not isinstance(text, str):
            raise TypeError("Text must be a string")

        if len(text.encode("utf-8")) > 500_000:
            raise ValueError("Text exceeds the 500,000-byte limit")

        # Bounded NLP windows with overlapping context preserve global Unicode
        # offsets. Resolve duplicate / conflicting spans only after all windows.
        if len(text) > 20_000:
            unique = {}
            start = 0
            while start < len(text):
                end = min(start + 20_000, len(text))
                for item in self.detect(text[start:end], entities, rules):
                    item = {**item, "start": item["start"] + start, "end": item["end"] + start}
                    key = (item["entity_type"], item["start"], item["end"])
                    if key not in unique or item["confidence"] > unique[key]["confidence"]:
                        unique[key] = item
                if end == len(text):
                    break
                start = end - 1_024
            return list(unique.values())

        # If specific categories were requested,
        # keep only categories currently supported by Veil.
        if entities is not None:
            entities = [
                entity
                for entity in entities
                if entity in self.SUPPORTED_ENTITIES
            ]

            # User selected no supported categories.
            if not entities:
                return []

        enabled = set(entities if entities is not None else self.SUPPORTED_ENTITIES)
        available = set(self.analyzer.get_supported_entities(language=self.language))
        builtin = sorted(enabled & available)
        results = self.analyzer.analyze(text=text.translate(str.maketrans("٠١٢٣٤٥٦٧٨٩۰۱۲۳۴۵۶۷۸۹", "01234567890123456789")), language=self.language, entities=builtin) if builtin else []
        detected = [
            {
                "entity_type": result.entity_type,
                "text": text[result.start:result.end],
                "start": result.start,
                "end": result.end,
                "confidence": round(result.score, 3),
                "source": "recognizer",
            }
            for result in sorted(
                results,
                key=lambda item: (item.start, item.end),
            )
        ]

        doc = None
        if enabled & {"ORGANIZATION", "INDIRECT_IDENTIFIER"}:
            doc = self.analyzer.nlp_engine.process_text(text, self.language).tokens
        return detected + arabic_entities(self.arabic, text, enabled) + suggestions(text, enabled, rules, doc, self.language)

    def suggest(
        self,
        text: str,
        entities: list[str] | None = None,
        rules: list[dict] | None = None,
        options: dict | None = None,
    ) -> list[dict]:

        options = {**validate_options(options, self.SUPPORTED_ENTITIES), "language": self.language}
        candidates = self.detect(text, entities, (rules or []) + options.get("session_rules", []))
        unique = {}
        for item in candidates:
            key = (item["entity_type"], item["start"], item["end"])
            if key not in unique or item["confidence"] > unique[key]["confidence"]:
                unique[key] = item
        candidates, self.suppressed = filter_candidates(text, list(unique.values()), options)

        ranked = sorted(
            candidates,
            key=lambda item: (
                item["entity_type"] in {"STREET_ADDRESS", "POSTAL_CODE", "INDIRECT_IDENTIFIER"},
                item["confidence"],
                item["end"] - item["start"],
            ),
            reverse=True,
        )

        selected = []
        starts = []
        for candidate in ranked:
            position = bisect_left(starts, candidate["start"])
            before = position > 0 and selected[position - 1]["end"] > candidate["start"]
            after = position < len(selected) and selected[position]["start"] < candidate["end"]
            if not before and not after:
                starts.insert(position, candidate["start"])
                selected.insert(position, candidate)
        return selected