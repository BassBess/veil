from presidio_analyzer import AnalyzerEngine
from presidio_analyzer.nlp_engine import NlpEngineProvider


class VeilDetector:
    SUPPORTED_ENTITIES = {
        "PERSON",
        "EMAIL_ADDRESS",
        "PHONE_NUMBER",
        "LOCATION",
        "ORGANIZATION",
    }

    def __init__(self):
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

    def detect(
        self,
        text: str,
        entities: list[str] | None = None,
    ) -> list[dict]:

        if not isinstance(text, str):
            raise TypeError("Text must be a string")

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

        results = self.analyzer.analyze(
            text=text,
            language="en",
            entities=entities,
        )

        return [
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

    def suggest(
        self,
        text: str,
        entities: list[str] | None = None,
    ) -> list[dict]:

        candidates = self.detect(text, entities)

        ranked = sorted(
            candidates,
            key=lambda item: (
                item["confidence"],
                item["end"] - item["start"],
            ),
            reverse=True,
        )

        selected = []

        for candidate in ranked:
            overlaps = any(
                candidate["start"] < existing["end"]
                and candidate["end"] > existing["start"]
                for existing in selected
            )

            if not overlaps:
                selected.append(candidate)

        return sorted(
            selected,
            key=lambda item: item["start"],
        )