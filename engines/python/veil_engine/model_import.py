"""Bounded, data-only imports. Network access lives in model_download.py."""
from contextlib import contextmanager
import hashlib
import io
import json
import os
from pathlib import Path
import stat
from zipfile import ZipFile, BadZipFile

from veil_engine.catalog import LANGUAGES
from veil_engine.components import EngineError
from veil_engine.model_specs import (MANIFEST, MAX_JSON, MODEL_SUFFIXES, adapter_metadata, adapter_spec,
                                     artifact_path, bounded_string, package_spec, tokenizer_files)
from veil_engine.model_store import BLOCK_SIZE, MAX_BUNDLE, MAX_FILE

TOKENIZER_LIMITS = {"BPE": 64 * 1024**2, "Unigram": 64 * 1024**2,
                    "WordPiece": 32 * 1024**2, "WordLevel": 32 * 1024**2}
MAX_TOKENIZER = max(TOKENIZER_LIMITS.values())
LEGACY_TOKENIZER_LIMIT = min(TOKENIZER_LIMITS.values())


def read_json(stream, limit=MAX_JSON):
    def unique(pairs):
        values = dict(pairs)
        if len(pairs) != len(values):
            raise EngineError("JSON metadata contains duplicate keys.")
        return values

    try:
        payload = stream.read(limit + 1)
        if len(payload) > limit:
            raise EngineError("JSON metadata exceeds the size limit.")
        return json.loads(payload, object_pairs_hook=unique)
    except (UnicodeError, json.JSONDecodeError, RecursionError):
        raise EngineError("Could not read valid model metadata.") from None


@contextmanager
def open_local(root, name):
    path = root / artifact_path(name)
    for part in path.relative_to(root).parts:
        root = root / part
        if root.is_symlink():
            raise EngineError("Model imports cannot contain symbolic links.")
    if not path.is_file():
        raise EngineError("A selected model file is missing or is not a regular file.")
    flags = os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0)
    flags |= getattr(os, "O_BINARY", 0)
    with os.fdopen(os.open(path, flags), "rb") as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise EngineError("Choose regular model files.")
        yield stream


def validate_tokenizer(data):
    try:
        config = read_json(io.BytesIO(data), MAX_TOKENIZER)
        model = config["model"]
        kind = model["type"]
        limit = TOKENIZER_LIMITS[kind]
    except (EngineError, KeyError, TypeError):
        raise EngineError("Use a supported Hugging Face JSON tokenizer.") from None
    if len(data) > limit:
        raise EngineError("The tokenizer exceeds the size limit for its format.")
    vocab = model.get("vocab")
    expected = list if kind == "Unigram" else dict
    if not isinstance(vocab, expected) or not 1 <= len(vocab) <= 1_000_000:
        raise EngineError("The tokenizer vocabulary is missing or exceeds the supported size.")
    if kind == "BPE":
        merges = model.get("merges")
        if not isinstance(merges, list) or len(merges) > 1_000_000:
            raise EngineError("The BPE merge table is missing or exceeds the supported size.")


def raw_metadata(name, version, license_name, directory, files, model_name, languages=()):
    result = {"name": bounded_string(name, "model name"), "version": version, "license": license_name,
              "adapters": []}
    if not model_name or model_name not in files or not model_name.endswith(".onnx") or "config.json" not in files:
        return result
    try:
        with open_local(directory, "config.json") as stream:
            config = read_json(stream)
        if not isinstance(config, dict):
            return result
        architectures = config.get("architectures")
        if (not isinstance(architectures, list) or not any(
                isinstance(value, str) and value.endswith("ForTokenClassification") for value in architectures)):
            return result
        labels = config.get("id2label")
        if not isinstance(labels, dict) or not 2 <= len(labels) <= 256:
            return result
        names = [labels[str(index)] for index in range(len(labels))]
        declared = config.get("language", languages)
        if isinstance(declared, str):
            declared = [declared]
        if not isinstance(declared, (list, tuple)):
            return result
        spec = {"adapter": "onnx-token-v1", "model": model_name, "tokenizer": "tokenizer.json",
                "labels": names, "languages": [lang for lang in LANGUAGES if lang in declared],
                "max_length": min(config.get("max_position_embeddings", 512), 512)}
        if (config.get("type_vocab_size") or 0) > 1:
            spec["inputs"] = {name: name for name in ("input_ids", "attention_mask", "token_type_ids")}
        # Validate the descriptor even when the optional tokenizer has yet to be restored.
        adapter_spec(spec, {*files, "tokenizer.json"}, allow_unresolved=True)
        result.update(adapter_metadata([spec], files))
    except (EngineError, KeyError, TypeError, ValueError):
        pass
    return result


def copy_artifacts(directory, paths, opener, checkpoint, progress, expected_hashes=None, sizes=None,
                   *, tokenizers=()):
    total_bytes = 0
    files = {}
    if not 1 <= len(paths) <= 32 or len({name.casefold() for name in paths}) != len(paths):
        raise EngineError("Choose 1–32 distinct model files.")
    reported_total = sum(sizes[name] for name in paths) if sizes and all(sizes.get(name) is not None for name in paths) else None
    for name in paths:
        artifact_path(name)
        checkpoint()
        is_tokenizer = name == "tokenizer.json" or name in tokenizers
        if is_tokenizer:
            limit, limit_error = MAX_TOKENIZER, "The tokenizer exceeds the installation size limit."
        elif name.endswith(".json"):
            limit, limit_error = MAX_JSON, "Model JSON metadata exceeds the size limit."
        else:
            limit, limit_error = MAX_FILE, "A model file exceeds the installation size limit."
        expected_size = sizes.get(name) if sizes else None
        if expected_size is not None:
            if expected_size <= 0:
                raise EngineError("A model file is empty.")
            if expected_size > limit:
                raise EngineError(limit_error)
        path = directory / name
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        size = 0
        digest = hashlib.sha256()
        with opener(name) as stream, path.open("xb") as output:
            while block := stream.read(BLOCK_SIZE):
                checkpoint()
                size += len(block)
                total_bytes += len(block)
                if size > limit:
                    raise EngineError(limit_error)
                if total_bytes > MAX_BUNDLE:
                    raise EngineError("The model exceeds the installation size limit.")
                output.write(block)
                digest.update(block)
                progress(total_bytes, reported_total, name)
            output.flush()
            os.fsync(output.fileno())
        sha = digest.hexdigest()
        expected_hash = expected_hashes.get(name) if expected_hashes else None
        if (not size or (expected_size is not None and size != expected_size)
                or (expected_hash is not None and sha != expected_hash)):
            raise EngineError("A model file is empty, incomplete, or has an incorrect checksum.")
        if is_tokenizer and size > LEGACY_TOKENIZER_LIMIT:
            validate_tokenizer(path.read_bytes())
            checkpoint()
        files[name] = {"size": size, "sha256": sha}
    return files


def install_local(store, path):
    path = Path(path)
    if path.is_symlink() or not path.exists():
        raise EngineError("Choose a local model file, directory, or Veil model package.")
    checkpoint = lambda: None
    progress = lambda *_: None
    try:
        with store.stage() as stage:
            directory = Path(stage)
            if path.is_file() and path.suffix.lower() == ".veilmodel":
                if path.stat().st_size > MAX_BUNDLE + MAX_JSON * 2:
                    raise EngineError("The Veil model archive exceeds the installation size limit.")
                with ZipFile(path) as archive:
                    entries = archive.infolist()
                    if not 2 <= len(entries) <= 33:
                        raise EngineError("A Veil package must contain a manifest and 1–32 files.")
                    seen = {}
                    for entry in entries:
                        name = artifact_path(entry.filename)
                        mode = stat.S_IFMT(entry.external_attr >> 16)
                        if (name.casefold() in seen or mode not in (0, stat.S_IFREG)
                                or entry.flag_bits & 1 or not 0 < entry.file_size <= MAX_FILE):
                            raise EngineError("The Veil model archive contains an unsafe or duplicate entry.")
                        seen[name.casefold()] = entry
                    if MANIFEST not in archive.namelist():
                        raise EngineError("The Veil archive is missing its model manifest.")
                    with archive.open(MANIFEST) as stream:
                        metadata, hashes = package_spec(read_json(stream))
                    if set(archive.namelist()) != {*hashes, MANIFEST}:
                        raise EngineError("Veil package contents must match the manifest.")
                    files = copy_artifacts(directory, hashes, archive.open, checkpoint, progress,
                                           hashes, {entry.filename: entry.file_size for entry in entries},
                                           tokenizers=tokenizer_files(metadata))
            else:
                root = path if path.is_dir() else path.parent
                descriptor = path.name == MANIFEST and path.is_file() or path.is_dir() and (root / MANIFEST).exists()
                if descriptor:
                    with open_local(root, MANIFEST) as stream:
                        metadata, hashes = package_spec(read_json(stream))
                    files = copy_artifacts(directory, hashes, lambda name: open_local(root, name),
                                           checkpoint, progress, hashes, tokenizers=tokenizer_files(metadata))
                else:
                    if path.is_dir():
                        weights = sorted(filter(lambda name: (root / name).is_file(),
                                                ("model.onnx", "onnx/model.onnx")))
                        if not weights:
                            weights = sorted(item.name for item in root.iterdir()
                                             if item.is_file() and item.suffix.lower() in MODEL_SUFFIXES)
                        if not weights:
                            raise EngineError("Choose a directory containing model weights or a Veil manifest.")
                        if len(weights) > 1:
                            raise EngineError("Choose a model file or provide a Veil manifest to resolve multiple weights.")
                        model_name = weights[0]
                    elif path.is_file() and path.suffix.lower() in MODEL_SUFFIXES:
                        model_name = path.name
                    else:
                        raise EngineError("Choose ONNX, GGUF, SafeTensors, a model directory, or a .veilmodel package.")
                    sidecars = ["config.json", "tokenizer.json", "README.md", "LICENSE", "LICENSE.txt"]
                    names = [model_name, *(name for name in sidecars if (root / name).is_file())]
                    files = copy_artifacts(directory, names, lambda name: open_local(root, name), checkpoint, progress)
                    metadata = raw_metadata(path.stem, "Unspecified", "Unspecified", directory, files, model_name)
            return store.publish(directory, metadata, files, None, checkpoint)
    except (BadZipFile, OSError, RuntimeError, NotImplementedError):
        raise EngineError("The local model could not be imported. Check its files and available storage.") from None
