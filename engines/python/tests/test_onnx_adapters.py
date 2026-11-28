"""Tiny generated graphs and a fictional vocabulary exercise the real optional runtime."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

from veil_engine.components import Capability, EngineError
from veil_engine.model_import import install_local
from veil_engine.model_store import Store
from veil_engine.service import Engine

HAS_ONNX = all(importlib.util.find_spec(name) for name in ("numpy", "onnx", "onnxruntime", "tokenizers"))


@unittest.skipUnless(HAS_ONNX, "Optional ONNX runtime dependencies are unavailable")
class OnnxTests(unittest.TestCase):
    def setUp(self):
        from onnx import TensorProto, helper
        from tokenizers import Tokenizer, models, pre_tokenizers

        self.helper = helper
        self.TensorProto = TensorProto
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.bundle = self.root / "fictional"
        self.bundle.mkdir()
        tokenizer = Tokenizer(models.WordLevel({"[UNK]": 0, "Mira": 1, "Vale": 2, "x": 3}, unk_token="[UNK]"))
        tokenizer.pre_tokenizer = pre_tokenizers.Whitespace()
        tokenizer.save(str(self.bundle / "tokenizer.json"))
        self.store = Store(self.root / "store")
        self.addCleanup(self.store.db.close)
        self.engine = Engine(self.store)

    def make_graph(self, nodes, inputs, outputs, initializer=(), **settings):
        return self.helper.make_model(
            self.helper.make_graph(nodes, "Synthetic fixture", inputs, outputs, initializer),
            ir_version=10, opset_imports=[self.helper.make_opsetid("", 18)], **settings
        ).SerializeToString()

    def fixture(self):
        h, t = self.helper, self.TensorProto
        vocabulary = h.make_tensor("lookup", t.FLOAT, [4, 3], [10, 0, 0, 0, 10, 0, 0, 0, 10, 10, 0, 0])
        tokens = self.make_graph(
            [h.make_node("Gather", ["lookup", "tokens"], ["ner"])],
            [h.make_tensor_value_info("tokens", t.INT64, [1, "n"])],
            [h.make_tensor_value_info("ner", t.FLOAT, [1, "n", 3])], [vocabulary],
        )
        context = self.make_graph(
            [h.make_node("Constant", [], ["classes"], value=h.make_tensor("scores", t.FLOAT, [1, 2], [0, 6]))],
            [h.make_tensor_value_info("ids", t.INT32, [1, "sequence"]),
             h.make_tensor_value_info("segments", t.INT32, [1, "sequence"])],
            [h.make_tensor_value_info("classes", t.FLOAT, [1, 2])],
        )
        (self.bundle / "tokens.onnx").write_bytes(tokens)
        (self.bundle / "context.onnx").write_bytes(context)
        manifest = {
            "schema": 1, "name": "Synthetic Veil model", "version": "1", "license": "CC0-1.0",
            "files": {item.name: hashlib.sha256(item.read_bytes()).hexdigest() for item in self.bundle.iterdir()},
            "adapters": [
                {"adapter": "onnx-token-v1", "languages": ["en"], "model": "tokens.onnx",
                 "tokenizer": "tokenizer.json", "labels": ["O", "B-PER", "I-PER"],
                 "inputs": {"input_ids": "tokens"}, "output": "ner", "max_length": 8, "stride": 3},
                {"adapter": "onnx-context-v1", "languages": ["en"], "model": "context.onnx",
                 "tokenizer": "tokenizer.json", "labels": ["O", "ORG"],
                 "inputs": {"input_ids": "ids", "token_type_ids": "segments"}, "output": "classes",
                 "max_length": 8, "context_input": "pair"},
            ],
        }
        (self.bundle / "veil-model.json").write_text(json.dumps(manifest))
        return manifest

    def test_local_token_and_context_model_through_active_profile(self):
        self.fixture()
        model = install_local(self.store, self.bundle)
        component = "model:" + model["id"]
        profile = {
            "schema": 1, "name": "Fictional pass", "policy": "static", "legacy_nlp": False,
            "languages": ["en"], "budget": {},
            "steps": [{"component": name, "capability": capability} for name, capability in [
                (component, Capability.DETECT), ("capitalized-phrases", Capability.GENERATE),
                (component, Capability.CLASSIFY), ("exact-span", Capability.FUSE), ("veil-ranking", Capability.RANK)]],
        }
        self.assertFalse(self.engine.dispatch({"action": "status"})["models"][0]["loaded"])
        saved = self.engine.dispatch({"action": "profile_save", "profile": profile})
        self.engine.dispatch({"action": "profile_activate", "id": saved["id"]})
        self.assertTrue(self.engine.dispatch({"action": "status"})["models"][0]["loaded"])
        document = "🙂 " + "x " * 12 + "Mira Vale met Nova Guild. Email mira@example.com"
        result = self.engine.dispatch({"action": "detect", "text": document,
                                       "entities": ["PERSON", "ORGANIZATION", "EMAIL_ADDRESS"]})
        self.assertEqual([(item["text"], item["entity_type"]) for item in result["detections"]],
                         [("Mira Vale", "PERSON"), ("Nova Guild", "ORGANIZATION"),
                          ("mira@example.com", "EMAIL_ADDRESS")])
        self.assertEqual(result["detections"][0]["start"], document.index("Mira"))
        for item in result["detections"]:
            self.assertEqual(item["text"], document[item["start"]:item["end"]])
            self.assertGreater(item["confidence"], 0.5)
        model_metrics = [item for item in self.engine.last_run["metrics"] if item["component"] == component]
        self.assertTrue(all(item["model_calls"] > 0 for item in model_metrics))
        self.engine.dispatch({"action": "model_unload", "id": model["id"]})
        self.assertFalse(self.engine.dispatch({"action": "status"})["models"][0]["loaded"])
        self.engine.dispatch({"action": "detect", "text": document, "entities": ["PERSON"]})
        self.assertTrue(self.engine.dispatch({"action": "status"})["models"][0]["loaded"])

    def test_declared_compatibility_is_verified_at_load(self):
        manifest = self.fixture()
        manifest["adapters"][0]["inputs"] = {"input_ids": "incorrect"}
        (self.bundle / "veil-model.json").write_text(json.dumps(manifest))
        model = install_local(self.store, self.bundle)
        with self.assertRaisesRegex(EngineError, "input names"):
            self.engine.dispatch({"action": "model_load", "id": model["id"]})
        self.assertFalse(self.engine.dispatch({"action": "status"})["models"][0]["loaded"])

    def test_native_runtime_rejects_external_data_and_custom_ops(self):
        from veil_engine.runtime import OnnxRuntime

        h, t = self.helper, self.TensorProto
        graph = self.make_graph(
            [h.make_node("Arbitrary", ["x"], ["y"], domain="third.party")],
            [h.make_tensor_value_info("x", t.FLOAT, [1])],
            [h.make_tensor_value_info("y", t.FLOAT, [1])],
        )
        with self.assertRaisesRegex(EngineError, "custom operators"):
            OnnxRuntime().open(graph)
        remote = h.make_tensor("external", t.FLOAT, [1], [1])
        remote.data_location = t.EXTERNAL
        item = remote.external_data.add()
        item.key, item.value = "location", "../external.bin"
        graph = self.make_graph(
            [], [], [h.make_tensor_value_info("external", t.FLOAT, [1])], [remote],
        )
        with self.assertRaisesRegex(EngineError, "external tensor data"):
            OnnxRuntime().open(graph)
