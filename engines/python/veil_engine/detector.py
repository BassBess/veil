from bisect import bisect_left
import spacy
from veil_engine.context_rules import suggestions
from tldextract import TLDExtract
from presidio_analyzer import AnalyzerEngine
from presidio_analyzer.predefined_recognizers import EmailRecognizer
from presidio_analyzer.nlp_engine import NlpEngineProvider


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

    def __init__(self):
        if not spacy.util.is_package("en_core_web_sm"):
            raise RuntimeError("The local en_core_web_sm model is missing. Install the Python requirements before using detection.")
        config = {
            "nlp_engine_name": "spacy",
            "models": [
                {
                    "lang_code": "en",
                    "model_name": "en_core_web_sm",
                }
            ],
        }

        provider = NlpEngineProvider(nlp_configuration=config)

        self.analyzer = AnalyzerEngine(
            nlp_engine=provider.create_engine(),
            supported_languages=["en"],
        )
        self.analyzer.registry.remove_recognizer("EmailRecognizer")
        self.analyzer.registry.add_recognizer(OfflineEmailRecognizer())

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
        available = set(self.analyzer.get_supported_entities(language="en"))
        builtin = sorted(enabled & available)
        results = self.analyzer.analyze(text=text, language="en", entities=builtin) if builtin else []
        detected = [
            {
                "entity_type": result.entity_type,
                "text": text[result.start:result.end],
                "start": result.start,
                "end": result.end,
                "confidence": round(result.score, 3),
            }
            for result in sorted(
                results,
                key=lambda item: (item.start, item.end),
            )
        ]

        doc = None
        if enabled & {"ORGANIZATION", "INDIRECT_IDENTIFIER"}:
            doc = self.analyzer.nlp_engine.process_text(text, "en").tokens
        return detected + suggestions(text, enabled, rules, doc)

    def suggest(
        self,
        text: str,
        entities: list[str] | None = None,
        rules: list[dict] | None = None,
    ) -> list[dict]:

        candidates = self.detect(text, entities, rules)

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