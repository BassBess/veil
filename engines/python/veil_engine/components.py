"""Contracts for trusted components. Offsets are Python Unicode character indices."""
from bisect import bisect_left
from dataclasses import dataclass, field
from enum import StrEnum
from math import isfinite
import re
from typing import Callable, NotRequired, Protocol, TypedDict


class Capability(StrEnum):
    DETECT = "span_detection"
    GENERATE = "candidate_generation"
    CLASSIFY = "contextual_classification"
    FUSE = "detection_fusion"
    RANK = "entity_ranking"
    MAP = "entity_mapping"
    ROUTE = "adaptive_routing"
    VERIFY = "privacy_verification"


class Region(TypedDict):
    start: int
    end: int


class Span(Region):
    entity_type: str
    text: str
    confidence: float
    source: NotRequired[str]
    reason: NotRequired[str]


@dataclass(frozen=True)
class Request:
    text: str
    language: str
    entities: frozenset[str]
    rules: list[dict]
    checkpoint: Callable[[], None] = field(repr=False)
    infer: Callable = field(repr=False)


class SpanDetector(Protocol):
    def detect(self, request: Request) -> list[Span]: ...


class CandidateGenerator(Protocol):
    def generate(self, request: Request) -> list[Region]: ...


class ContextualClassifier(Protocol):
    def classify(self, request: Request, regions: list[Region]) -> list[Span]: ...


class DetectionFusion(Protocol):
    def fuse(self, spans: list[Span]) -> list[Span]: ...


class EntityRanker(Protocol):
    def rank(self, spans: list[Span]) -> list[Span]: ...


class EntityMapper(Protocol):
    def map_entity(self, entity: Span, source: str) -> str: ...


class InferenceRouter(Protocol):
    def route(self, step: "Step") -> bool: ...


class PrivacyVerifier(Protocol):
    def verify(self, request: Request, spans: list[Span]) -> None: ...


METHODS = {
    Capability.DETECT: "detect", Capability.GENERATE: "generate",
    Capability.CLASSIFY: "classify", Capability.FUSE: "fuse",
    Capability.RANK: "rank", Capability.MAP: "map_entity",
    Capability.ROUTE: "route", Capability.VERIFY: "verify",
}


class EngineError(ValueError):
    """A controlled message suitable for display without document content."""


@dataclass(frozen=True)
class Step:
    component: str
    capability: Capability


@dataclass(frozen=True)
class Component:
    id: str
    name: str
    handlers: dict[Capability, Callable]
    mandatory: bool = False
    languages: frozenset[str] | None = None

    def describe(self):
        return {"id": self.id, "name": self.name, "capabilities": list(self.handlers),
                "mandatory": self.mandatory, "languages": sorted(self.languages) if self.languages else None}


class Registry:
    def __init__(self):
        self.components: dict[str, Component] = {}

    def copy(self):
        registry = Registry()
        registry.components.update(self.components)
        return registry

    def register(self, component_id: str, name: str, implementation: object,
                 *capabilities: Capability, mandatory=False, languages=None):
        if component_id in self.components or not capabilities:
            raise EngineError("Duplicate component or missing capabilities.")
        handlers = {capability: getattr(implementation, METHODS[capability], None)
                    for capability in capabilities}
        if not all(map(callable, handlers.values())):
            raise EngineError("Component does not implement its declared capabilities.")
        self.components[component_id] = Component(component_id, name, handlers, mandatory,
                                                  frozenset(languages) if languages else None)

    def resolve(self, step: Step):
        component = self.components.get(step.component)
        if component is None or step.capability not in component.handlers:
            raise EngineError("A profile component is missing or cannot perform the selected task.")
        return component.handlers[step.capability]


class ExactFusion:
    def fuse(self, spans):
        unique = {}
        for item in spans:
            key = (item["entity_type"], item["start"], item["end"])
            if key not in unique or item["confidence"] > unique[key]["confidence"]:
                unique[key] = item
        return list(unique.values())


class VeilRanking:
    def rank(self, spans):
        return sorted(spans, key=lambda item: (
            item["entity_type"] in {"STREET_ADDRESS", "POSTAL_CODE", "INDIRECT_IDENTIFIER"},
            item["confidence"], item["end"] - item["start"],
        ), reverse=True)


class ConfidenceRanking:
    def rank(self, spans):
        return sorted(spans, key=lambda item: (
            item["confidence"], item["end"] - item["start"],
        ), reverse=True)


def select_nonoverlapping(ranked, reserved=()):
    # Resolve model overlaps separately to keep the existing core suggestions intact.
    if reserved:
        starts = [span["start"] for span in reserved]
        remaining = []
        for candidate in ranked:
            position = bisect_left(starts, candidate["end"])
            if not position or reserved[position - 1]["end"] <= candidate["start"]:
                remaining.append(candidate)
        return sorted([*reserved, *select_nonoverlapping(remaining)], key=lambda item: item["start"])

    selected, starts = [], []
    for candidate in ranked:
        position = bisect_left(starts, candidate["start"])
        before = position > 0 and selected[position - 1]["end"] > candidate["start"]
        after = position < len(selected) and selected[position]["start"] < candidate["end"]
        if not before and not after:
            starts.insert(position, candidate["start"])
            selected.insert(position, candidate)
    return selected


class StaticRouter:
    def route(self, step):
        return True


class OffsetVerifier:
    def verify(self, request, spans):
        for span in spans:
            start, end = span["start"], span["end"]
            if (type(start) is not int or type(end) is not int
                    or not 0 <= start < end <= len(request.text)
                    or span["entity_type"] not in request.entities
                    or not isfinite(span["confidence"]) or not 0 <= span["confidence"] <= 1
                    or span["text"] != request.text[start:end]):
                raise EngineError("A component returned an invalid sensitivity span.")


class CapitalizedPhrases:
    _pattern = re.compile(r"[^\W\d_]+(?:[-'’][^\W\d_]+)*")

    def generate(self, request):
        regions = []
        first = last = None
        count = 0
        for match in self._pattern.finditer(request.text):
            joins = (last is not None and match.start() - last <= 3
                     and request.text[last:match.start()].strip(" \t") == "")
            if count and (not joins or count == 4 or not match[0][0].isupper()):
                if count >= 2:
                    regions.append({"start": first, "end": last})
                count = 0
            if match[0][0].isupper():
                if not count:
                    first = match.start()
                last = match.end()
                count += 1
        if count >= 2:
            regions.append({"start": first, "end": last})
        if len(regions) > 4096:
            raise EngineError("Too many contextual candidates. Split the document into smaller files.")
        return regions


def algorithms():
    registry = Registry()
    registry.register("exact-span", "Exact span fusion", ExactFusion(), Capability.FUSE)
    registry.register("veil-ranking", "Veil category priority", VeilRanking(), Capability.RANK)
    registry.register("confidence-ranking", "Confidence first", ConfidenceRanking(), Capability.RANK)
    registry.register("capitalized-phrases", "Capitalized phrases", CapitalizedPhrases(), Capability.GENERATE,
                      languages=("en", "fr", "es"))
    registry.register("static", "Static routing", StaticRouter(), Capability.ROUTE, mandatory=True)
    registry.register("source-offsets", "Source offset verification", OffsetVerifier(),
                      Capability.VERIFY, mandatory=True)
    return registry
