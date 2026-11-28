"""Run an entire, verified detection request or return an error with no spans."""
from veil_engine.catalog import LANGUAGES, SUPPORTED_ENTITIES
from veil_engine.components import (Capability, EngineError, Request, Step, algorithms, select_nonoverlapping)
from veil_engine.controller import Budget, Controller
from veil_engine.detection_options import filter_candidates, validate_options
from veil_engine.labels import is_custom
from veil_engine.profiles import DEFAULT_STEPS, custom_categories, steps_from

CORE = Step("shared-core", Capability.DETECT)
CORE_NAME = "Shared detection"
ROUTER = Step("static", Capability.ROUTE)
VERIFY = Step("source-offsets", Capability.VERIFY)


class SharedCore:
    def __init__(self, legacy_nlp=True):
        self.legacy_nlp = legacy_nlp

    def detect(self, request):
        from veil_engine.detector import VeilDetector
        from veil_engine.languages import ModelUnavailable

        try:
            return VeilDetector(request.language, legacy_nlp=self.legacy_nlp).detect(
                request.text, list(request.entities), request.rules)
        except ModelUnavailable as error:
            raise EngineError(str(error)) from None


class ModelComponent:
    def __init__(self, store, sessions, model_id):
        self.store, self.sessions, self.model_id = store, sessions, model_id

    def detect(self, request):
        return self.sessions.load(self.store, self.model_id).detect(request)

    def classify(self, request, regions):
        return self.sessions.load(self.store, self.model_id).classify(request, regions)


def run_detection(text, entities, language, rules=None, options=None, *, profile=None, store=None,
                  sessions=None, metrics=None, cancelled=lambda: False, registry=None, core=None):
    if not isinstance(text, str) or len(text.encode("utf-8")) > 500_000:
        raise EngineError("Text exceeds the 500,000-byte limit or is invalid.")
    if not isinstance(language, str) or language not in LANGUAGES:
        raise EngineError("Choose English, French, Spanish, or Arabic.")
    if profile and language not in profile["languages"]:
        raise EngineError("The active profile does not support this document language.")
    model_categories = custom_categories(profile, store)
    if entities is not None and any(is_custom(value) and value not in model_categories for value in entities):
        raise EngineError("A selected custom category is unavailable in the active profile.")
    available = SUPPORTED_ENTITIES | model_categories
    enabled = available if entities is None else available.intersection(entities)
    try:
        options = {**validate_options(options, SUPPORTED_ENTITIES), "language": language}
    except ValueError as error:
        raise EngineError(str(error)) from None

    registry = registry.copy() if registry is not None else algorithms()
    steps = steps_from(profile) if profile else ()
    for step in steps:
        if step.component.startswith("model:") and step.component not in registry.components:
            model_id = step.component.removeprefix("model:")
            model = store.model(model_id)
            registry.register(step.component, model["name"], ModelComponent(store, sessions, model_id),
                              *(Capability(spec["capability"]) for spec in model["adapters"] if spec["capability"]))
    registry.register(CORE.component, CORE_NAME,
                      core if core is not None else SharedCore(profile is None or profile["legacy_nlp"]),
                      Capability.DETECT, mandatory=True)
    controller = Controller(Budget.parse(profile["budget"]) if profile else Budget(),
                            registry.resolve(ROUTER), cancelled)

    def invoke(step, *args):
        return controller.invoke(step, registry.resolve(step), *args,
                                 mandatory=registry.components[step.component].mandatory)

    request = Request(text, language, frozenset(enabled), (rules or []) + options.get("session_rules", []),
                      controller.checkpoint, controller.infer)
    try:
        baseline = invoke(CORE, request)
        invoke(VERIFY, request, baseline)
        core_fusion, core_ranking = DEFAULT_STEPS
        baseline, suppressed = filter_candidates(text, invoke(core_fusion, baseline), options)
        baseline = select_nonoverlapping(invoke(core_ranking, baseline))
        regions = {}
        additions = []
        for step in steps:
            match step.capability:
                case Capability.DETECT:
                    additions.extend(invoke(step, request))
                case Capability.GENERATE:
                    for region in invoke(step, request):
                        start, end = region["start"], region["end"]
                        if type(start) is not int or type(end) is not int or not 0 <= start < end <= len(text):
                            raise EngineError("A component returned an invalid contextual region.")
                        regions[start, end] = region
                case Capability.CLASSIFY:
                    additions.extend(invoke(step, request, list(regions.values())))
                case Capability.FUSE:
                    invoke(VERIFY, request, additions)
                    additions, rejected = filter_candidates(text, invoke(step, additions), options)
                    suppressed = merge_suppressions(suppressed, rejected, baseline)
                case Capability.RANK:
                    additions = invoke(step, additions)
        result = select_nonoverlapping(additions, baseline) if additions else baseline
        invoke(VERIFY, request, result)
        return {"detections": result, "suppressed": suppressed}
    except EngineError:
        raise
    except Exception:
        raise EngineError("A local component failed. Detection did not complete.") from None
    finally:
        if metrics is not None:
            metrics.extend(controller.report())


def merge_suppressions(baseline, added, accepted):
    def key(item):
        return item["entity_type"], item["start"], item["end"]

    seen = {key(item) for item in [*baseline, *accepted]}
    return [*baseline, *(item for item in added if key(item) not in seen)]
