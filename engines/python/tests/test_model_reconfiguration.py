import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from veil_engine.components import Capability, EngineError
from veil_engine.labels import resolve_labels
from veil_engine.model_download import repair
from veil_engine.model_import import copy_artifacts
from veil_engine.model_store import Store
from veil_engine.runtime import span, trim_token_span
from veil_engine.service import Engine

REPO = "example/fictional-model"
REVISION = "a" * 40
WEIGHTS = "onnx/fixture.onnx"
DATA = b"fictional weights"
CONFIG = json.dumps({
    "architectures": ["FixtureForTokenClassification"],
    "id2label": {"0": "O", "1": "B-PER", "2": "I-PER", "3": "B-FIXTURE_HANDLE", "4": "I-FIXTURE_HANDLE"},
}).encode()
TOKENIZER = b'{"model":{"type":"BPE","vocab":{"fixture":0},"merges":[]}}'
CONTENTS = {WEIGHTS: DATA, "config.json": CONFIG, "tokenizer.json": TOKENIZER}


def file_entry(data):
    return {"sha256": hashlib.sha256(data).hexdigest(), "size": len(data)}


def profile(model):
    return {"schema": 1, "name": "Fictional custom categories", "policy": "static", "budget": {},
            "legacy_nlp": False, "languages": ["en", "fr", "ar"],
            "steps": [{"component": component, "capability": capability} for component, capability in (
                ("model:" + model, "span_detection"), ("exact-span", "detection_fusion"),
                ("veil-ranking", "entity_ranking"))]}


class LabelTests(unittest.TestCase):
    def test_aliases_unresolved_custom_and_ignore_preserve_label_index_and_boundary(self):
        native = ["O", "B-PER", "I-PER", "B-POSTCODE", "S-FIXTURE_HANDLE", "B-MEDICAL_ID", "I-MEDICAL_ID"]
        labels, groups = resolve_labels(native, Capability.DETECT, {})
        self.assertEqual(labels, ["O", "B-PERSON", "I-PERSON", "B-POSTAL_CODE", None, "B-CUSTOM_MEDICAL_ID", "I-CUSTOM_MEDICAL_ID"])
        self.assertEqual([group["label"] for group in groups], ["PER", "POSTCODE", "FIXTURE_HANDLE", "MEDICAL_ID"])
        labels, groups = resolve_labels(native, Capability.DETECT, {"FIXTURE_HANDLE": "CUSTOM_SECRET", "MEDICAL_ID": "@ignore"})
        self.assertEqual(labels[-3:], ["U-CUSTOM_SECRET", "O", "O"])
        self.assertEqual(groups[-1]["target"], "@ignore")
        for mapping in [{"MISSING": "PERSON"}, {"FIXTURE_HANDLE": None}, {"FIXTURE_HANDLE": "CUSTOM_bad"}, {"FIXTURE_HANDLE": "O"}]:
            with self.subTest(mapping=mapping), self.assertRaises(EngineError):
                resolve_labels(native, Capability.DETECT, mapping)
        context, _ = resolve_labels(["O", "email", "credentials"], Capability.CLASSIFY,
                                    {"CREDENTIALS": "CUSTOM_SECRET"})
        self.assertEqual(context, ["O", "EMAIL_ADDRESS", "CUSTOM_SECRET"])

    def test_token_offsets_trim_separator_and_keep_combining_marks(self):
        source = "😀  Jose\u0301, ثم"
        original = span(source, 1, 7, "PERSON", .9)
        trimmed = trim_token_span(source, original)
        self.assertEqual((trimmed["text"], trimmed["start"], trimmed["end"]), ("Jose\u0301", 3, 8))
        self.assertIsNone(trim_token_span(source, span(source, 1, 3, "PERSON", .9)))


class TokenizerLimitTests(unittest.TestCase):
    def test_large_limit_depends_on_role_and_tokenizer_format(self):
        with tempfile.TemporaryDirectory() as temp, patch.multiple(
                "veil_engine.model_import", LEGACY_TOKENIZER_LIMIT=128, MAX_TOKENIZER=256,
                MAX_JSON=128, TOKENIZER_LIMITS={"BPE": 256, "WordLevel": 128}):
            root = Path(temp)
            data = TOKENIZER.ljust(180, b" ")
            def install(directory, payload, name, tokenizers=()):
                directory.mkdir()
                return copy_artifacts(directory, [name], lambda _: io.BytesIO(payload),
                                      lambda: None, lambda *_: None, tokenizers=tokenizers)
            name = "assets/vocab.json"
            installed = install(root / "valid", data, name, (name,))
            self.assertEqual(installed[name], file_entry(data))
            for index, (value, kind, roles, message) in enumerate([
                    (data, name, (), "metadata exceeds"),
                    (data.replace(b"BPE", b"WordLevel"), name, (name,), "limit for its format"),
                    (data.replace(b"BPE", b"Unknown"), "tokenizer.json", (), "supported Hugging Face JSON tokenizer"),
                    (data.ljust(257, b" "), "tokenizer.json", (), "installation size limit")]):
                with self.subTest(index=index), self.assertRaisesRegex(EngineError, message):
                    install(root / str(index), value, kind, roles)


class FakeHub:
    def __init__(self, tokenizer=TOKENIZER):
        files = {**CONTENTS, "tokenizer.json": tokenizer}
        info = {"sha": REVISION, "cardData": {"language": ["en", "fr", "ar"]},
                "siblings": [{"rfilename": name, "size": len(data), "lfs": {"sha256": file_entry(data)["sha256"]}}
                             for name, data in CONTENTS.items()]}
        self.responses = {f"/api/models/{REPO}/revision/{REVISION}?blobs=true": json.dumps(info).encode(),
                          **{f"/{REPO}/resolve/{REVISION}/{name}": data for name, data in files.items()}}
        self.requests = []

    def open(self, path):
        self.requests.append(path)
        return io.BytesIO(self.responses[path])


class WorkflowTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.store = Store(Path(temp.name))
        self.addCleanup(self.store.db.close)
        with self.store.stage() as directory:
            path = Path(directory)
            (path / WEIGHTS).parent.mkdir()
            (path / WEIGHTS).write_bytes(DATA)
            (path / "config.json").write_bytes(CONFIG)
            old = {"name": REPO, "version": REVISION, "license": "CC0-1.0", "adapters": [],
                   "source": {"repository": REPO, "revision": REVISION},
                   "compatibility_note": "Legacy tokenizer limit"}
            old = self.store.publish(path, old, {WEIGHTS: file_entry(DATA), "config.json": file_entry(CONFIG)},
                                     None, lambda: None)
        self.identifier = old["id"]
        self.weights = self.store.models / self.identifier / WEIGHTS
        self.engine = Engine(self.store)

    def perform_repair(self, hub, authorized=True, checkpoint=lambda: None, progress=lambda *_: None):
        return repair(self.store, self.identifier, authorized, checkpoint, progress, hub)

    def test_repair_reuses_verified_artifacts_and_preserves_label_decisions_and_profile(self):
        hub = FakeHub()
        self.perform_repair(hub)
        self.assertEqual(hub.requests[1:], [f"/{REPO}/resolve/{REVISION}/tokenizer.json"])
        self.assertEqual(self.weights.read_bytes(), DATA)
        config = self.engine.dispatch({"action": "model_configuration", "id": self.identifier})
        adapter, = config["adapters"]
        self.assertEqual(adapter["unresolved"], 1)
        self.assertEqual(adapter["missing"], [])
        with self.assertRaisesRegex(EngineError, "no supported adapter"):
            self.engine.profiles.save(profile(self.identifier))
        with self.assertRaisesRegex(EngineError, "no supported Veil adapter"):
            self.engine.sessions.load(self.store, self.identifier)

        ignored, = self.engine.dispatch({"action": "model_configure", "id": self.identifier,
                                         "mappings": {"onnx-token-v1": {"PER": "@ignore", "FIXTURE_HANDLE": "@ignore"}}})
        self.assertEqual((ignored["unresolved"], ignored["ignored"]), (0, 2))
        self.assertFalse(self.store.model(self.identifier)["adapters"])
        update = {"action": "model_configure", "id": self.identifier,
                  "mappings": {"onnx-token-v1": {"FIXTURE_HANDLE": "CUSTOM_SECRET"}}}
        self.engine.dispatch(update)
        self.assertEqual(self.store.model(self.identifier)["adapters"][0]["labels"][-2:],
                         ["B-CUSTOM_SECRET", "I-CUSTOM_SECRET"])
        # Repair a corrupt file using its original verified digest.
        self.weights.write_bytes(b"broken")
        hub = FakeHub()
        self.perform_repair(hub)
        self.assertEqual(hub.requests[1:], [f"/{REPO}/resolve/{REVISION}/{WEIGHTS}"])
        self.assertEqual(self.weights.read_bytes(), DATA)
        self.assertEqual(self.engine.dispatch({"action": "model_configuration", "id": self.identifier})
                         ["adapters"][0]["overrides"], update["mappings"]["onnx-token-v1"])
        self.engine.sessions.loaded[self.identifier] = object()
        saved = self.engine.profiles.save(profile(self.identifier))
        self.engine.profiles.activate(saved["id"])
        self.assertEqual(self.engine.dispatch({"action": "status"})["custom_categories"], ["CUSTOM_SECRET"])
        with self.assertRaisesRegex(EngineError, "active profile"):
            self.engine.dispatch(update)
        with self.assertRaisesRegex(EngineError, "active profile"):
            self.perform_repair(FakeHub())
        reopened_store = Store(self.store.root)
        self.addCleanup(reopened_store.db.close)
        reopened = Engine(reopened_store)
        self.assertEqual(reopened.profiles.active_id(), saved["id"])
        self.assertEqual(reopened.dispatch({"action": "status"})["custom_categories"], ["CUSTOM_SECRET"])

    def test_unauthorized_failed_and_cancelled_repairs_do_not_publish_files(self):
        fake = FakeHub()
        original = self.store.model(self.identifier)
        with self.assertRaisesRegex(EngineError, "Authorize"):
            self.perform_repair(fake, authorized=False)
        self.assertFalse(fake.requests)
        with self.assertRaisesRegex(EngineError, "incorrect checksum"):
            self.perform_repair(FakeHub(b" " * len(TOKENIZER)))
        progress = []
        def checkpoint():
            if progress and progress[-1][0]:
                raise EngineError("Model download cancelled.")
        with self.assertRaisesRegex(EngineError, "cancelled"):
            self.perform_repair(FakeHub(), checkpoint=checkpoint, progress=lambda *event: progress.append(event))
        self.assertEqual(self.store.model(self.identifier), original)
        self.assertFalse((self.weights.parents[1] / "tokenizer.json").exists())
        self.assertFalse(list(self.store.models.glob(".install-*")))
