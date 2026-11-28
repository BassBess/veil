"""Versioned detection recipes. The shared core always runs outside user steps."""
import json
from uuid import uuid4

from veil_engine.catalog import LANGUAGES
from veil_engine.components import Capability, EngineError, Step
from veil_engine.controller import Budget
from veil_engine.labels import group_label, is_custom
from veil_engine.model_specs import bounded_string
from veil_engine.model_store import canonical

ORDER = (Capability.DETECT, Capability.GENERATE, Capability.CLASSIFY, Capability.FUSE, Capability.RANK)
DEFAULT_STEPS = (Step("exact-span", Capability.FUSE), Step("veil-ranking", Capability.RANK))


def steps_from(document):
    return [Step(item["component"], Capability(item["capability"])) for item in document["steps"]]


def model_refs(profile):
    return {step.component.removeprefix("model:") for step in steps_from(profile)
            if step.component.startswith("model:")}


def custom_categories(profile, store):
    categories = set()
    if profile:
        for step in steps_from(profile):
            if not step.component.startswith("model:"):
                continue
            model = store.model(step.component.removeprefix("model:"))
            for adapter in model["adapters"]:
                if adapter["capability"] == step.capability:
                    for label in adapter["labels"]:
                        _, group = group_label(label, step.capability == Capability.DETECT)
                        if is_custom(group):
                            categories.add(group)
    return categories


def validate_profile(document, store, registry):
    fields = {"name", "schema", "policy", "budget", "legacy_nlp", "languages", "steps"}
    if (not isinstance(document, dict) or set(document) != fields or type(document["schema"]) is not int
            or document["schema"] != 1 or document["policy"] != "static"):
        raise EngineError("Use a schema 1 Veil profile with the static execution policy.")
    name = bounded_string(document["name"], "profile name", 80)
    languages = document["languages"]
    if (not isinstance(languages, list) or not 1 <= len(languages) <= len(LANGUAGES)
            or not all(isinstance(lang, str) and lang in LANGUAGES for lang in languages)
            or len(set(languages)) != len(languages)):
        raise EngineError("Choose the profile's supported document languages.")
    if type(document["legacy_nlp"]) is not bool:
        raise EngineError("Choose whether to use the legacy language models.")
    if not isinstance(document["steps"], list) or not 2 <= len(document["steps"]) <= 32:
        raise EngineError("A profile needs 2–32 steps.")
    steps, stages = set(), []
    for raw in document["steps"]:
        if (not isinstance(raw, dict) or set(raw) != {"component", "capability"}
                or not isinstance(raw["component"], str) or not isinstance(raw["capability"], str)
                or raw["capability"] not in ORDER):
            raise EngineError("The profile has an invalid step or unsupported task.")
        step = Step(raw["component"], Capability(raw["capability"]))
        if step in steps:
            raise EngineError("A profile cannot repeat a component for the same capability.")
        steps.add(step)
        stages.append(step.capability)
        if step.component.startswith("model:"):
            model = store.model(step.component.removeprefix("model:"))
            specs = [spec for spec in model["adapters"] if spec["capability"] == step.capability]
            if not specs:
                raise EngineError("A selected model has no supported adapter for the requested task.")
            if not set(languages).issubset(specs[0]["languages"]):
                raise EngineError("A selected model does not support every profile language.")
        else:
            registry.resolve(step)
            supported = registry.components[step.component].languages
            if supported and not set(languages).issubset(supported):
                raise EngineError("A selected algorithm does not support every profile language.")
    if (stages != sorted(stages, key=ORDER.index) or stages.count(Capability.FUSE) != 1
            or stages.count(Capability.RANK) != 1):
        raise EngineError("Order steps as detection, candidate generation, classification, fusion, and ranking.")
    if (Capability.CLASSIFY in stages) != (Capability.GENERATE in stages):
        raise EngineError("Contextual classification requires both a candidate generator and a classifier.")
    return {**document, "name": name, "budget": vars(Budget.parse(document["budget"]))}


class Profiles:
    def __init__(self, store, registry, sessions):
        self.store = store
        self.registry = registry
        self.sessions = sessions

    def listing(self):
        return [{"id": row[0], **json.loads(row[1])}
                for row in self.store.db.execute("SELECT id,data FROM profiles ORDER BY rowid")]

    def get(self, profile_id):
        if not isinstance(profile_id, str) or len(profile_id) != 32:
            raise EngineError("Choose an existing profile.")
        row = self.store.db.execute("SELECT data FROM profiles WHERE id = ?", (profile_id,)).fetchone()
        if not row:
            raise EngineError("Choose an existing profile.")
        return json.loads(row[0])

    def active_id(self):
        row = self.store.db.execute("SELECT profile_id FROM active WHERE singleton = 1").fetchone()
        return row[0] if row else None

    def validate(self, document):
        return validate_profile(document, self.store, self.registry)

    def _check_dependencies(self, document):
        if document["legacy_nlp"]:
            from veil_engine.languages import ModelUnavailable, require_legacy_language
            try:
                for language in document["languages"]:
                    require_legacy_language(language)
            except ModelUnavailable as error:
                raise EngineError(str(error)) from None
        previous = set(self.sessions.loaded)
        try:
            for model_id in sorted(model_refs(document)):
                self.sessions.load(self.store, model_id)
        except Exception:
            for model_id in self.sessions.loaded.keys() - previous:
                self.sessions.unload(model_id)
            raise

    def save(self, document, profile_id=None):
        with self.store.write():
            document = self.validate(document)
            if profile_id is None:
                profile_id = uuid4().hex
                self.store.db.execute("INSERT INTO profiles VALUES (?, ?)", (profile_id, canonical(document)))
            else:
                self.get(profile_id)
                if self.active_id() == profile_id:
                    self._check_dependencies(document)
                self.store.db.execute("UPDATE profiles SET data = ? WHERE id = ?", (canonical(document), profile_id))
        return {"id": profile_id, **document}

    def activate(self, profile_id):
        with self.store.write():
            if profile_id is None:
                self.store.db.execute("DELETE FROM active")
            else:
                profile = self.validate(self.get(profile_id))
                self._check_dependencies(profile)
                self.store.db.execute("INSERT OR REPLACE INTO active VALUES (1, ?)", (profile_id,))
        return profile_id

    def delete(self, profile_id):
        with self.store.write():
            self.get(profile_id)
            if self.active_id() == profile_id:
                raise EngineError("Switch to another profile or use Veil's existing detection before deleting this profile.")
            self.store.db.execute("DELETE FROM profiles WHERE id = ?", (profile_id,))
