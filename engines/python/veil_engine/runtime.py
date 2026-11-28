"""Local model runtime contracts and capability-specific ONNX adapters."""
from dataclasses import dataclass
import hashlib
import os
from typing import Protocol
from unicodedata import category as unicode_category

from veil_engine.components import Capability, EngineError
from veil_engine.model_import import validate_tokenizer

ONNX_HELP = "Install the optional ONNX dependencies from requirements-onnx.txt."


class ModelRuntime(Protocol):
    def open(self, graph: bytes): ...


class OnnxRuntime:
    def open(self, graph):
        # Set the opt-out before importing ORT so its native uploader never initializes.
        os.environ["ORT_DISABLE_TELEMETRY"] = "1"
        try:
            import onnx
            import onnxruntime as ort
            ort.disable_telemetry_events()
        except (ImportError, OSError):
            raise EngineError(ONNX_HELP) from None

        def validate(message):
            if isinstance(message, onnx.TensorProto) and (message.external_data or message.data_location):
                raise EngineError("ONNX external tensor data is unsupported.")
            if isinstance(message, (onnx.NodeProto, onnx.OperatorSetIdProto)):
                if message.domain not in ("", "ai.onnx", "ai.onnx.ml"):
                    raise EngineError("ONNX custom operators are unsupported.")
            for field, value in message.ListFields():
                if field.message_type:
                    for child in value if field.is_repeated else (value,):
                        validate(child)

        try:
            model = onnx.load_model_from_string(graph)
            if model.functions or model.training_info:
                raise EngineError("ONNX local functions and training graphs are unsupported.")
            validate(model)
            del model
            options = ort.SessionOptions()
            options.intra_op_num_threads = 2
            options.inter_op_num_threads = 1
            options.log_severity_level = 4
            return ort.InferenceSession(graph, sess_options=options, providers=["CPUExecutionProvider"])
        except EngineError:
            raise
        except Exception:
            raise EngineError("The local ONNX runtime could not open this graph.") from None


class LogitAdapter:
    def __init__(self, spec, graph, tokenizer_json, runtime):
        try:
            import numpy as np
            from tokenizers import Tokenizer
        except ImportError:
            raise EngineError(ONNX_HELP) from None
        self.np = np
        self.spec = spec
        validate_tokenizer(tokenizer_json)
        try:
            self.tokenizer = Tokenizer.from_str(tokenizer_json.decode("utf-8"))
            self.tokenizer.no_padding()
            self.tokenizer.no_truncation()
        except Exception:
            raise EngineError("The tokenizer must be a compatible tokenizer.json.") from None
        self.session = runtime.open(graph)
        self.labels = spec["labels"]
        self.dtypes = {}
        inputs = {tensor.name: tensor for tensor in self.session.get_inputs()}
        if set(inputs) != set(spec["inputs"].values()):
            raise EngineError("ONNX input names do not match the adapter descriptor.")
        integer_types = {"tensor(int64)": np.int64, "tensor(int32)": np.int32}
        for key, name in spec["inputs"].items():
            tensor = inputs[name]
            shape = tensor.shape
            if not shape or len(shape) != 2 or (isinstance(shape[0], int) and shape[0] != 1):
                raise EngineError("ONNX inputs must accept batch size one and token sequences.")
            if tensor.type not in integer_types or isinstance(shape[1], int):
                raise EngineError("ONNX inputs must use dynamic token axes with int32 or int64 tensors.")
            self.dtypes[key] = integer_types[tensor.type]
        output = next((item for item in self.session.get_outputs() if item.name == spec["output"]), None)
        rank = 3 if spec["capability"] == Capability.DETECT else 2
        if (output is None or output.type not in ("tensor(float)", "tensor(float16)", "tensor(double)")
                or not output.shape or len(output.shape) != rank
                or (isinstance(output.shape[-1], int) and output.shape[-1] != len(self.labels))):
            raise EngineError("ONNX logits do not match this adapter task and label map.")

    def predict(self, encoding):
        np = self.np
        values = {"input_ids": encoding.ids, "attention_mask": encoding.attention_mask,
                  "token_type_ids": encoding.type_ids}
        if not encoding.ids or len(encoding.ids) > self.spec["max_length"]:
            raise EngineError("A token sequence exceeds the model adapter limit.")
        try:
            feed = {name: np.asarray([values[key]], dtype=self.dtypes[key])
                    for key, name in self.spec["inputs"].items()}
            logits, = self.session.run([self.spec["output"]], feed)
        except Exception:
            raise EngineError("A local ONNX model failed during inference.") from None
        expected = ((1, len(encoding.ids), len(self.labels)) if self.spec["capability"] == Capability.DETECT
                    else (1, len(self.labels)))
        if logits.shape != expected or not np.isfinite(logits).all():
            raise EngineError("The model returned invalid ONNX logits.")
        scores = np.exp(logits[0].astype(np.float64) - logits[0].max(axis=-1, keepdims=True))
        scores /= scores.sum(axis=-1, keepdims=True)
        return scores.argmax(axis=-1), scores.max(axis=-1)


class TokenAdapter(LogitAdapter):
    def __init__(self, *args):
        super().__init__(*args)
        available = self.spec["max_length"] - self.tokenizer.num_special_tokens_to_add(False)
        if available <= self.spec["stride"]:
            raise EngineError("The token overlap leaves no room for model input.")
        self.tokenizer.enable_truncation(max_length=self.spec["max_length"], stride=self.spec["stride"])
        probe = self.tokenizer.encode("Veil checks fictional text.")
        if not any(end > start for start, end in probe.offsets):
            raise EngineError("The tokenizer did not produce usable offsets.")
        self.predict(probe)

    def detect(self, request):
        if not request.text.strip():
            return []
        encoded = self.tokenizer.encode(request.text)
        spans = []
        for window in (encoded, *encoded.overflowing):
            labels, scores = request.infer(self.predict, window)
            for candidate in self._decode(request, window, labels, scores):
                trimmed = trim_token_span(request.text, candidate)
                if trimmed is not None:
                    spans.append(trimmed)
        return spans

    def _decode(self, request, window, labels, scores):
        active = None
        for (start, end), special, label, score in zip(window.offsets, window.special_tokens_mask,
                                                      labels, scores, strict=True):
            marker, _, category = self.labels[label].partition("-")
            if special or start == end or category not in request.entities or score < self.spec["min_confidence"]:
                if active:
                    yield active
                    active = None
                continue
            if not 0 <= start < end <= len(request.text):
                raise EngineError("The tokenizer returned invalid original-text offsets.")
            if (active and marker in {"I", "L"} and category == active["entity_type"]
                    and (start <= active["end"] or request.text[active["end"]:start].isspace())):
                active["end"] = max(active["end"], end)
                active["text"] = request.text[active["start"]:active["end"]]
                active["confidence"] = min(active["confidence"], float(score))
            else:
                if active:
                    yield active
                active = span(request.text, start, end, category, score)
            if marker in {"L", "U"}:
                yield active
                active = None
        if active:
            yield active


class ContextAdapter(LogitAdapter):
    def __init__(self, *args):
        super().__init__(*args)
        self.predict(self.encode("Fictional Name", "A fictional example."))

    def encode(self, candidate, context):
        pair = self.spec["context_input"] == "pair"
        if len(self.tokenizer.encode(candidate, "" if pair else None).ids) > self.spec["max_length"]:
            raise EngineError("A contextual candidate exceeds the model token limit.")
        while True:
            encoded = self.tokenizer.encode(candidate, context if pair else None)
            if len(encoded.ids) <= self.spec["max_length"]:
                return encoded
            context = context[len(context) // 4 + 1: -(len(context) // 4 + 1)]

    def classify(self, request, regions):
        spans = []
        margin = self.spec["context_chars"]
        for region in regions:
            request.checkpoint()
            start, end = region["start"], region["end"]
            context = request.text[max(0, start - margin):end + margin]
            labels, scores = request.infer(self.predict, self.encode(request.text[start:end], context))
            category = self.labels[int(labels)]
            if category in request.entities and scores >= self.spec["min_confidence"]:
                spans.append(span(request.text, start, end, category, scores))
        return spans


def trim_token_span(source, item):
    # Some byte-level tokenizers include a separator in the first or last token's offset.
    start, end = item["start"], item["end"]
    while start < end and source[start].isspace():
        start += 1
    while end > start and source[end - 1].isspace():
        end -= 1
    if start == end:
        return None
    while end < len(source) and unicode_category(source[end]).startswith("M"):
        end += 1
    return {**item, "start": start, "end": end, "text": source[start:end]}


def span(source, start, end, category, score):
    return {"entity_type": category, "start": start, "end": end, "text": source[start:end],
            "confidence": float(score), "source": "local model"}


FACTORIES = {"onnx-token-v1": TokenAdapter, "onnx-context-v1": ContextAdapter}


@dataclass
class LoadedModel:
    adapters: dict

    def detect(self, request):
        return self.adapters[Capability.DETECT].detect(request)

    def classify(self, request, regions):
        return self.adapters[Capability.CLASSIFY].classify(request, regions)


class ModelSessions:
    def __init__(self, runtime=None):
        self.loaded = {}
        self.generations = {}
        self.runtime = runtime if runtime is not None else OnnxRuntime()

    def is_loaded(self, model):
        model_id = model["id"]
        generation = model.get("generation", 0)
        return model_id in self.loaded and self.generations.get(model_id, generation) == generation

    def load(self, store, model_id):
        model = store.model(model_id)
        if not self.is_loaded(model):
            self.unload(model_id)
            adapters = {}
            for spec in model["adapters"]:
                factory = FACTORIES.get(spec["adapter"])
                if factory:
                    adapters[spec["capability"]] = factory(
                        spec, self.read_verified(store, model, spec["model"]),
                        self.read_verified(store, model, spec["tokenizer"]), self.runtime,
                    )
            if not adapters:
                raise EngineError("This model has no supported Veil adapter.")
            self.loaded[model_id] = LoadedModel(adapters)
            self.generations[model_id] = model.get("generation", 0)
        return self.loaded[model_id]

    def unload(self, model_id):
        self.loaded.pop(model_id, None)
        self.generations.pop(model_id, None)

    @staticmethod
    def read_verified(store, model, name):
        entry = model["files"][name]
        try:
            with (store.models / model["id"] / name).open("rb") as stream:
                data = stream.read(entry["size"] + 1)
        except OSError:
            raise EngineError("An installed model file is missing. Repair or reinstall the model.") from None
        if len(data) != entry["size"] or hashlib.sha256(data).hexdigest() != entry["sha256"]:
            raise EngineError("An installed model file has changed. Repair or reinstall the model.")
        return data
