"""Strict, versioned descriptor for data-only model bundles."""
from pathlib import PurePosixPath
import re

from veil_engine.catalog import LANGUAGES
from veil_engine.components import Capability, EngineError
from veil_engine.labels import resolve_labels

MANIFEST = "veil-model.json"
HASH = re.compile(r"[0-9a-f]{64}")
MAX_JSON = 2 * 1024**2
ARTIFACT_SUFFIXES = {".onnx", ".json", ".gguf", ".safetensors", ".txt", ".md"}
MODEL_SUFFIXES = {".onnx", ".gguf", ".safetensors"}
ADAPTER_TASKS = {"onnx-token-v1": Capability.DETECT, "onnx-context-v1": Capability.CLASSIFY}


def artifact_path(name):
    if (not isinstance(name, str) or not re.fullmatch(r"[a-zA-Z0-9_.\-/]{1,240}", name)
            or any(not part or part.startswith(".") or part.endswith(".")
                   or re.fullmatch(r"(?i)(?:con|prn|aux|nul|com[0-9]|lpt[0-9])(?:\..*)?", part)
                   for part in name.split("/"))
            or (PurePosixPath(name).suffix.lower() not in ARTIFACT_SUFFIXES and name != "LICENSE")):
        raise EngineError("Model packages must use relative paths and supported data file types.")
    return name


def bounded_string(value, label, maximum=160):
    if (not isinstance(value, str) or not value.strip() or len(value) > maximum
            or any(ord(char) < 32 or ord(char) == 127 for char in value)):
        raise EngineError(f"Invalid {label}.")
    return value.strip()


def integer(value, low, high, label):
    if type(value) is not int or not low <= value <= high:
        raise EngineError(f"Invalid {label}.")
    return value


def adapter_spec(spec, files, *, allow_unresolved=False, approved=None, defaults=None):
    if not isinstance(spec, dict):
        raise EngineError("Invalid model adapter descriptor.")
    adapter = spec.get("adapter")
    if not isinstance(adapter, str):
        raise EngineError("Invalid adapter identifier.")
    if adapter not in ADAPTER_TASKS:
        # Future adapters remain inspectable and cannot be selected by a profile.
        return {"adapter": bounded_string(adapter, "adapter identifier"), "capability": None}

    allowed = {"adapter", "model", "tokenizer", "inputs", "output", "labels", "languages",
               "max_length", "stride", "min_confidence", "context_chars", "context_input", "label_map"}
    if set(spec) - allowed:
        raise EngineError("The model adapter contains unknown fields.")
    model = artifact_path(spec.get("model"))
    tokenizer = artifact_path(spec.get("tokenizer"))
    if model not in files or not model.endswith(".onnx") or tokenizer not in files or not tokenizer.endswith(".json"):
        raise EngineError("The adapter needs listed ONNX and tokenizer JSON files.")
    inputs = spec.get("inputs", {"input_ids": "input_ids", "attention_mask": "attention_mask"})
    if (not isinstance(inputs, dict) or set(inputs) - {"input_ids", "attention_mask", "token_type_ids"}
            or "input_ids" not in inputs):
        raise EngineError("Invalid ONNX input mapping.")
    inputs = {key: bounded_string(value, "tensor name") for key, value in inputs.items()}
    if len(set(inputs.values())) != len(inputs):
        raise EngineError("ONNX input tensor names must be distinct.")
    labels = spec.get("labels")
    if not isinstance(labels, list) or not 2 <= len(labels) <= 256:
        raise EngineError("Provide ONNX labels in output-index order.")
    task = ADAPTER_TASKS[adapter]
    names = [bounded_string(label, "class label") for label in labels]
    normalized, _ = resolve_labels(names, task, spec.get("label_map", {}), approved=approved, defaults=defaults)
    if not allow_unresolved and (None in normalized or all(label == "O" for label in normalized)):
        raise EngineError("Resolve the model labels and map at least one sensitive category.")
    languages = spec.get("languages")
    if (not isinstance(languages, list) or not languages or len(languages) > len(LANGUAGES)
            or not all(isinstance(lang, str) and lang in LANGUAGES for lang in languages)):
        raise EngineError("Declare the languages supported by each model adapter.")
    max_length = integer(spec.get("max_length", 512), 8, 4096, "model token limit")
    confidence = spec.get("min_confidence", 0.5)
    if type(confidence) not in (float, int) or not 0 <= confidence <= 1:
        raise EngineError("Invalid model confidence threshold.")
    result = {"adapter": adapter, "capability": task, "model": model, "tokenizer": tokenizer,
              "inputs": inputs, "output": bounded_string(spec.get("output", "logits"), "tensor name"),
              "labels": normalized, "languages": sorted(set(languages)), "max_length": max_length,
              "min_confidence": confidence}
    if task == Capability.DETECT:
        if {"context_chars", "context_input"} & set(spec):
            raise EngineError("Context options require a contextual classifier.")
        result["stride"] = integer(spec.get("stride", min(64, max_length // 4)),
                                   0, max_length - 5, "token overlap")
    else:
        if "stride" in spec:
            raise EngineError("Token overlap requires a token classifier.")
        context_input = spec.get("context_input", "pair")
        if context_input not in ("pair", "candidate"):
            raise EngineError("Choose pair or candidate contextual input.")
        result["context_input"] = context_input
        result["context_chars"] = integer(spec.get("context_chars", 120), 0, 1024, "context length")
    return result


def package_spec(manifest):
    if (not isinstance(manifest, dict) or type(manifest.get("schema")) is not int or manifest["schema"] != 1
            or set(manifest) - {"schema", "name", "version", "license", "files", "adapters"}):
        raise EngineError("Use a Veil model manifest with schema 1.")
    files = manifest.get("files")
    if not isinstance(files, dict) or not 1 <= len(files) <= 32 or MANIFEST in files:
        raise EngineError("List 1–32 data files with SHA-256 hashes in the model manifest.")
    if any(not isinstance(digest, str) or not HASH.fullmatch(digest) for digest in files.values()):
        raise EngineError("The model manifest contains an invalid SHA-256 hash.")
    for name in files:
        artifact_path(name)
    if len({name.casefold() for name in files}) != len(files):
        raise EngineError("Model file paths must be unique on all supported systems.")
    specs = manifest.get("adapters", [])
    if not isinstance(specs, list) or len(specs) > 8:
        raise EngineError("Invalid model adapter list.")
    adapters = [adapter_spec(adapter, files, allow_unresolved=True) for adapter in specs]
    tasks = [adapter["capability"] for adapter in adapters if adapter["capability"]]
    if len(set(tasks)) != len(tasks):
        raise EngineError("A model can provide at most one adapter for each capability.")
    candidates = [spec if resolved["capability"] else {"adapter": resolved["adapter"]}
                  for spec, resolved in zip(specs, adapters, strict=True)]
    return {"name": bounded_string(manifest.get("name"), "model name"),
            "version": bounded_string(manifest.get("version"), "model version"),
            "license": bounded_string(manifest.get("license"), "model license"),
            **adapter_metadata(candidates, files)}, files


def adapter_metadata(candidates, files, *, approved=None, defaults=None):
    adapters = []
    for candidate in candidates:
        task = ADAPTER_TASKS.get(candidate["adapter"])
        if not task:
            adapters.append(adapter_spec(candidate, files))
        elif candidate["model"] in files and candidate["tokenizer"] in files:
            spec = adapter_spec(candidate, files, allow_unresolved=True, approved=approved,
                                defaults=(defaults or {}).get(candidate["adapter"]))
            if None not in spec["labels"] and any(label != "O" for label in spec["labels"]):
                adapters.append(spec)
    return {"adapter_candidates": candidates, "adapters": adapters}


def tokenizer_files(metadata):
    return {candidate["tokenizer"] for candidate in metadata.get("adapter_candidates", [])
            if candidate["adapter"] in ADAPTER_TASKS}
