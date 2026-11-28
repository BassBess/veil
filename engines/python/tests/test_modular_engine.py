import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from zipfile import ZipFile

from veil_engine.components import Capability as Cap, EngineError, Registry, Step, algorithms
from veil_engine.model_download import download, parse_source
from veil_engine.model_import import install_local
from veil_engine.model_specs import MANIFEST
from veil_engine.model_store import Store
from veil_engine.pipeline import run_detection
from veil_engine.profiles import Profiles
from veil_engine.runtime import ModelSessions, span
from veil_engine.service import Engine


def step(component, capability):
    return {"component": component, "capability": capability}


def profile(*steps, legacy=True, languages=("en",), budget=None):
    return {
        "name": "Fictional project", "schema": 1, "policy": "static",
        "legacy_nlp": legacy, "languages": list(languages), "budget": budget or {},
        "steps": [*steps, step("exact-span", Cap.FUSE), step("veil-ranking", Cap.RANK)],
    }


def spec(task="onnx-token-v1", **settings):
    return {
        "adapter": task, "model": "fixture.onnx", "tokenizer": "tokenizer.json",
        "labels": ["O", "B-PER"] if task == "onnx-token-v1" else ["O", "PERSON"],
        "languages": ["en"], **settings,
    }


def declaration(artifacts, adapters):
    return {"schema": 1, "name": "Fictional adapter", "version": "1.0", "license": "CC0-1.0",
            "files": {name: hashlib.sha256(data).hexdigest() for name, data in artifacts.items()},
            "adapters": adapters}


class FictionalClassifier:
    def detect(self, request):
        return [span(request.text, 2, 11, "PERSON", 0.99)]

    def classify(self, request, regions):
        return [span(request.text, region["start"], region["end"], "PERSON", 0.8) for region in regions]


class FictionalCore:
    def detect(self, request):
        start = request.text.index("mira@example.test")
        return [span(request.text, start, start + len("mira@example.test"), "EMAIL_ADDRESS", 0.9)]


class RegistryAndPipelineTests(unittest.TestCase):
    def test_multiple_capabilities_and_bad_registration(self):
        registry = Registry()
        registry.register("test", "Example classifier", FictionalClassifier(), Cap.DETECT, Cap.CLASSIFY)
        self.assertTrue(callable(registry.resolve(Step("test", Cap.CLASSIFY))))
        with self.assertRaises(EngineError):
            registry.resolve(Step("test", Cap.GENERATE))
        with self.assertRaises(EngineError):
            registry.register("test", "Example classifier", FictionalClassifier(), Cap.DETECT)
        with self.assertRaises(EngineError):
            registry.register("bad", "Wrong signature", FictionalClassifier(), Cap.RANK)

    def test_static_execution_runs_neural_even_with_core_match(self):
        document = "🙂 Mira Vale at mira@example.test"
        registry = algorithms()
        registry.register("fictional", "Fictional adapter", FictionalClassifier(), Cap.DETECT, Cap.CLASSIFY)
        metrics = []
        result = run_detection(document, ["PERSON", "EMAIL_ADDRESS"], "en",
                               profile=profile(step("fictional", Cap.DETECT),
                                               step("capitalized-phrases", Cap.GENERATE),
                                               step("fictional", Cap.CLASSIFY)),
                               core=FictionalCore(), registry=registry, metrics=metrics)
        self.assertEqual([item["text"] for item in result["detections"]], ["Mira Vale", "mira@example.test"])
        self.assertEqual(result["detections"][0]["start"], 2)
        calls = [(item["component"], item["capability"]) for item in metrics if item["invocations"]]
        self.assertIn(("fictional", Cap.DETECT), calls)
        self.assertIn(("fictional", Cap.CLASSIFY), calls)
        self.assertNotIn(document, json.dumps(metrics))

    def test_profile_models_cannot_hide_a_baseline_match(self):
        class Overlap:
            def detect(self, request):
                return [span(request.text, 0, len(request.text), "PERSON", 1)]

        registry = algorithms()
        registry.register("overlap", "Overlapping model", Overlap(), Cap.DETECT)
        result = run_detection("Mira at mira@example.test", ["PERSON", "EMAIL_ADDRESS"], "en",
                               profile=profile(step("overlap", Cap.DETECT)), core=FictionalCore(), registry=registry)
        self.assertEqual([span["entity_type"] for span in result["detections"]], ["EMAIL_ADDRESS"])

    def test_all_failures_abort_without_returning_partial_results(self):
        class BadClassifier:
            def detect(self, request):
                raise RuntimeError(request.text)

        class BadOffsets:
            def detect(self, request):
                return [span("wrong text", 0, 5, "PERSON", 1)]

        for component, budget, cancelled, error in [
            (BadClassifier(), {}, False, "did not complete"),
            (BadOffsets(), {}, False, "invalid sensitivity span"),
            (FictionalClassifier(), {"max_invocations": 1}, False, "invocation budget"),
            (FictionalClassifier(), {}, True, "cancelled"),
        ]:
            with self.subTest(component=type(component).__name__, budget=budget, cancelled=cancelled):
                registry = algorithms()
                registry.register("fictional", "Example", component, Cap.DETECT)
                metrics = []
                document = "Fictional mira@example.test"
                with self.assertRaisesRegex(EngineError, error) as caught:
                    run_detection(document, ["PERSON", "EMAIL_ADDRESS"], "en",
                                  profile=profile(step("fictional", Cap.DETECT), budget=budget),
                                  core=FictionalCore(), registry=registry, metrics=metrics,
                                  cancelled=lambda: cancelled)
                self.assertTrue(any(item["failures"] for item in metrics))
                self.assertNotIn(document, str(caught.exception) + json.dumps(metrics))

    def test_shared_checksums_and_literal_rules_run_without_legacy_models(self):
        result = run_detection("🙂 عنوان: حي السرو؛ 4111 1111 1111 1111",
                               ["CREDIT_CARD", "STREET_ADDRESS", "PERSON"], "ar",
                               rules=[{"entity_type": "PERSON", "phrase": "السرو"}],
                               profile=profile(legacy=False, languages=("ar",)))
        self.assertIn("4111 1111 1111 1111", [item["text"] for item in result["detections"]])
        self.assertIn("حي السرو", [item["text"] for item in result["detections"]])


class StoreTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.store = Store(self.root / "engine")
        self.addCleanup(self.store.db.close)
        self.artifacts = {"fixture.onnx": b"mock graph", "tokenizer.json": b"{}"}

    def bundle(self, manifest=None, extras=None):
        data = manifest if manifest is not None else declaration(self.artifacts, [spec()])
        path = self.root / "example.veilmodel"
        with ZipFile(path, "w") as archive:
            archive.writestr(MANIFEST, json.dumps(data))
            for name, content in {**self.artifacts, **(extras or {})}.items():
                archive.writestr(name, content)
        return path

    def test_local_install_verify_hashes_and_lifecycle(self):
        path = self.bundle()
        installed = install_local(self.store, path)
        self.assertEqual(install_local(self.store, path)["id"], installed["id"])
        self.assertEqual(len(self.store.installed()), 1)
        self.assertEqual(installed["size"], sum(map(len, self.artifacts.values())))
        self.assertEqual(installed["license"], "CC0-1.0")
        for name, data in self.artifacts.items():
            self.assertEqual(ModelSessions.read_verified(self.store, installed, name), data)
        model_path = self.store.models / installed["id"] / "fixture.onnx"
        model_path.write_bytes(b"changed")
        with self.assertRaisesRegex(EngineError, "changed"):
            ModelSessions.read_verified(self.store, installed, "fixture.onnx")
        self.store.remove(installed["id"])
        self.assertEqual(self.store.installed(), [])
        self.assertFalse(model_path.parent.exists())

    def test_unknown_adapter_raw_import_and_unsafe_archives(self):
        raw = self.root / "fixture.gguf"
        raw.write_bytes(b"fictional GGUF")
        model = install_local(self.store, raw)
        with self.assertRaisesRegex(EngineError, "no supported"):
            ModelSessions().load(self.store, model["id"])
        future = declaration(self.artifacts, [{"adapter": "future-v2"}])
        self.assertIsNone(install_local(self.store, self.bundle(future))["adapters"][0]["capability"])
        count = len(self.store.installed())
        modified = declaration(self.artifacts, [spec()])
        modified["files"]["fixture.onnx"] = "0" * 64
        with self.assertRaisesRegex(EngineError, "checksum"):
            install_local(self.store, self.bundle(modified))
        with self.assertRaisesRegex(EngineError, "relative paths"):
            install_local(self.store, self.bundle(extras={"../payload.txt": b"x"}))
        with self.assertRaisesRegex(EngineError, "must match"):
            install_local(self.store, self.bundle(extras={"unlisted.txt": b"x"}))
        self.assertEqual(len(self.store.installed()), count)
        self.assertFalse(list(self.store.models.glob(".install-*")))

    def test_profiles_validate_order_tasks_languages_and_active_dependencies(self):
        model = install_local(self.store, self.bundle())
        model_step = step("model:" + model["id"], Cap.DETECT)
        sessions = ModelSessions()
        sessions.loaded[model["id"]] = FictionalClassifier()
        manager = Profiles(self.store, algorithms(), sessions)
        self.assertEqual(manager.listing(), [])
        valid = profile(model_step)
        for invalid in [profile(model_step, model_step), profile(model_step, languages=("ar",)),
                        profile(step(model_step["component"], Cap.CLASSIFY)),
                        profile(step("static", Cap.ROUTE)),
                        profile(step("capitalized-phrases", Cap.GENERATE)),
                        {**valid, "steps": list(reversed(valid["steps"]))}]:
            with self.assertRaises(EngineError):
                manager.save(invalid)
        original = manager.save(valid)
        self.assertEqual(manager.get(original["id"])["name"], valid["name"])
        duplicate = manager.save({**manager.get(original["id"]), "name": "Fictional copy"})
        self.assertNotEqual(duplicate["id"], original["id"])
        manager.activate(original["id"])
        reopened = Store(self.store.root)
        self.addCleanup(reopened.db.close)
        engine = Engine(reopened, sessions)
        status = engine.dispatch({"action": "status"})
        self.assertEqual(status["active_profile"], original["id"])
        self.assertEqual(len(status["profiles"]), 2)
        with self.assertRaisesRegex(EngineError, "active profile"):
            engine.dispatch({"action": "model_remove", "id": model["id"]})
        with self.assertRaisesRegex(EngineError, "before deleting"):
            manager.delete(original["id"])
        exported = engine.dispatch({"action": "profile_export", "id": original["id"]})
        target = self.root / "profile.json"
        target.write_text(json.dumps(exported))
        self.assertNotEqual(engine.dispatch({"action": "profile_import", "path": target})["id"], original["id"])
        manager.activate(None)
        self.store.remove(model["id"])
        with self.assertRaisesRegex(EngineError, "installed model"):
            manager.activate(duplicate["id"])

    def test_activation_failure_preserves_active_profile(self):
        model = install_local(self.store, self.bundle())
        sessions = ModelSessions()
        manager = Profiles(self.store, algorithms(), sessions)
        first = manager.save(profile())
        second = manager.save(profile(step("model:" + model["id"], Cap.DETECT)))
        manager.activate(first["id"])
        with patch.object(sessions, "load", side_effect=EngineError("Incompatible fixture")), self.assertRaisesRegex(
                EngineError, "Incompatible fixture"):
            manager.activate(second["id"])
        self.assertEqual(manager.active_id(), first["id"])
        with patch.object(sessions, "load", side_effect=EngineError("Incompatible fixture")), self.assertRaises(EngineError):
            manager.save({**manager.get(second["id"]), "name": "Replacement"}, first["id"])
        self.assertEqual(manager.get(first["id"])["name"], first["name"])

    def test_download_requires_permission_and_caches_by_pinned_revision(self):
        commit = "a" * 40
        descriptor = json.dumps(declaration(self.artifacts, [spec()])).encode()
        repo = {"sha": commit, "siblings": [{"rfilename": name, "size": len(data)}
                for name, data in {**self.artifacts, MANIFEST: descriptor, "model.py": b"unsafe"}.items()]}
        paths = {"/api/models/author/fixture/revision/main?blobs=true": json.dumps(repo).encode()}
        paths.update({f"/author/fixture/resolve/{commit}/{name}": data
                      for name, data in {**self.artifacts, MANIFEST: descriptor}.items()})
        requests = []
        def open_fake(path):
            requests.append(path)
            return io.BytesIO(paths[path])
        hub = type("FakeHub", (), {"open": staticmethod(open_fake)})()
        with self.assertRaisesRegex(EngineError, "Authorize"):
            download(self.store, "author/fixture", False, lambda: None, lambda *_: None, hub)
        self.assertEqual(requests, [])

        received = []
        def cancel():
            if received and received[-1][0] > 0:
                raise EngineError("Model download cancelled.")
        with self.assertRaisesRegex(EngineError, "cancelled"):
            download(self.store, "author/fixture", True, cancel, lambda *args: received.append(args), hub)
        self.assertEqual(self.store.installed(), [])
        self.assertFalse(list(self.store.models.glob(".install-*")))

        model = download(self.store, "author/fixture", True, lambda: None, lambda *_: None, hub)
        before = len(requests)
        same = download(self.store, "https://huggingface.co/author/fixture", True, lambda: None, lambda *_: None, hub)
        self.assertEqual(model["id"], same["id"])
        self.assertEqual(len(requests), before + 1)  # Revision check, no artifact downloads.
        self.assertEqual(model["source"]["revision"], commit)

    def test_failed_download_publishes_nothing(self):
        class Unavailable:
            def open(self, path):
                raise EngineError("The fixture network failed.")
        with self.assertRaisesRegex(EngineError, "network failed"):
            download(self.store, "author/fixture", True, lambda: None, lambda *_: None, Unavailable())
        self.assertFalse(self.store.installed())

    def test_huggingface_urls_are_restricted(self):
        self.assertEqual(parse_source("https://huggingface.co/author/fixture/blob/test/onnx/tiny.onnx"),
                         ("author/fixture", "test", "onnx/tiny.onnx"))
        for url in ["https://other.example/author/fixture", "http://huggingface.co/author/fixture",
                    "https://huggingface.co:443/author/fixture", "https://user@huggingface.co/author/fixture",
                    "https://huggingface.co/a/b/resolve/main/code.py", "a/../../other"]:
            with self.assertRaises(EngineError):
                parse_source(url)


class WorkerTests(unittest.TestCase):
    def test_audit_guard_blocks_python_networking(self):
        completed = subprocess.run(
            [sys.executable, "-c", "import socket; from veil_engine.service import offline; "
             "offline(); socket.getaddrinfo('example.test', 443)"],
            capture_output=True, timeout=15,
        )
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn(b"Network access is disabled", completed.stderr)

    def test_worker_default_and_cancellation_keep_failures_content_free(self):
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / "cancel"
            marker.touch()
            document = "🙂 Fictional author mira@example.com"
            requests = [
                {"action": "status"},
                {"action": "detect", "text": document, "entities": ["EMAIL_ADDRESS"], "cancel_path": str(marker)},
                {"action": "status"},
                {"action": "detect", "text": document, "entities": ["EMAIL_ADDRESS"]},
                {"action": "status"},
            ]
            worker = subprocess.run(
                [sys.executable, "-m", "veil_engine.service", "serve", directory],
                input="".join(json.dumps(item) + "\n" for item in requests),
                capture_output=True, text=True, timeout=30,
            )
            self.assertEqual(worker.returncode, 0, worker.stderr)
            first, cancelled, failed, result, latest = map(json.loads, worker.stdout.splitlines())
            self.assertIsNone(first["result"]["active_profile"])
            self.assertNotIn("result", cancelled)
            self.assertIn("cancelled", cancelled["error"])
            self.assertFalse(failed["result"]["last_run"]["complete"])
            self.assertEqual(result["result"]["detections"][0]["text"], "mira@example.com")
            self.assertTrue(latest["result"]["last_run"]["complete"])
            self.assertNotIn(document, json.dumps(latest))
            self.assertNotIn("mira@example.com", json.dumps(latest))
