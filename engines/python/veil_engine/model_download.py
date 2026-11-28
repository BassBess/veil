"""Explicit, anonymous downloads of model data. This module receives no documents."""
from contextlib import contextmanager
from pathlib import Path
import hashlib
import re
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

from veil_engine.components import EngineError
from veil_engine.model_import import MAX_TOKENIZER, copy_artifacts, open_local, raw_metadata, read_json
from veil_engine.model_configuration import resolved_metadata
from veil_engine.model_store import MAX_BUNDLE
from veil_engine.model_specs import HASH, MANIFEST, MODEL_SUFFIXES, artifact_path, bounded_string, package_spec, tokenizer_files

REPO_SEGMENT = re.compile(r"[a-zA-Z0-9][a-zA-Z0-9_.-]{0,95}")
REVISION = re.compile(r"[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}")
COMMIT = re.compile(r"[a-f0-9]{40}")
HUB_HOST = "huggingface.co"
# First-party HTTPS download endpoints from the Hub's documentation and meta.json.
# Keep this list local and exact: the published firewall list also covers uploads.
ARTIFACT_HOSTS = frozenset({
    "cdn-lfs.huggingface.co",
    "cdn-lfs.hf.co",
    "cdn-lfs-us-1.hf.co",
    "cdn-lfs-eu-1.hf.co",
    "cas-bridge.xethub.hf.co",
    "transfer.xethub.hf.co",
    "transfer.xethub-eu.hf.co",
    "aws.cdn.hf.co",
    "us.aws.cdn.hf.co",
    "us-east-1.aws.cdn.hf.co",
    "us-west-2.aws.cdn.hf.co",
    "eu-west-3.aws.cdn.hf.co",
    "ap-southeast-1.aws.cdn.hf.co",
    "us.gcp.cdn.hf.co",
    "us-east1.us.gcp.cdn.hf.co",
    "us-central1.us.gcp.cdn.hf.co",
    "us-west4.us.gcp.cdn.hf.co",
    "europe-west4.us.gcp.cdn.hf.co",
    "asia-southeast1.us.gcp.cdn.hf.co",
})


def parse_source(source):
    if not isinstance(source, str) or len(source) > 640 or not source.isascii():
        raise EngineError("Enter a Hugging Face repository ID or URL.")
    value = source.strip()
    revision, selected = "main", None
    if "://" in value:
        parsed = urlsplit(value)
        if (parsed.scheme != "https" or parsed.netloc.lower() != HUB_HOST
                or parsed.query not in ("", "download=true") or parsed.fragment):
            raise EngineError("Use an HTTPS huggingface.co model URL.")
        parts = parsed.path.strip("/").split("/")
        if len(parts) >= 4 and parts[2] in ("tree", "resolve", "blob"):
            mode, revision = parts[2:4]
            if mode != "tree" and len(parts) > 4:
                selected = artifact_path("/".join(parts[4:]))
                if Path(selected).suffix.lower() not in MODEL_SUFFIXES:
                    raise EngineError("Choose a model weight file or the repository URL.")
            elif mode != "tree" or len(parts) != 4:
                raise EngineError("Choose a repository, revision, or model weight URL.")
            parts = parts[:2]
    else:
        parts = value.split("/")
    if (len(parts) not in (1, 2) or not all(REPO_SEGMENT.fullmatch(p) for p in parts)
            or not REVISION.fullmatch(revision)):
        raise EngineError("Enter a valid Hugging Face repository ID and revision.")
    return "/".join(parts), revision, selected


class ModelRedirects(HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        target = urlsplit(newurl)
        host = target.hostname
        if (target.scheme != "https" or (host != HUB_HOST and host not in ARTIFACT_HOSTS)
                or target.netloc.lower() not in (host, f"{host}:443")):
            raise EngineError("Hugging Face returned an unsupported download destination.")
        return super().redirect_request(request, fp, code, message, headers, newurl)


class Hub:
    def __init__(self):
        self.opener = build_opener(ProxyHandler({}), ModelRedirects())

    @contextmanager
    def open(self, path):
        try:
            request = Request(f"https://{HUB_HOST}{path}", headers={"User-Agent": "Veil Model Library"})
            with self.opener.open(request, timeout=15) as response:
                yield response
        except HTTPError as error:
            if error.code in (401, 403, 404):
                raise EngineError("The public Hugging Face repository or model file is unavailable.") from None
            raise EngineError(f"Hugging Face returned HTTP {error.code}. Try again later.") from None
        except (URLError, TimeoutError):
            raise EngineError("Could not complete the Hugging Face download. Check your connection.") from None


def download(store, source, authorized, checkpoint, progress, hub=None):
    if authorized is not True:
        raise EngineError("Authorize the model download first.")
    try:
        repo, revision, selected = parse_source(source)
    except ValueError as error:
        if isinstance(error, EngineError):
            raise
        raise EngineError("Enter a valid Hugging Face URL.") from None
    hub = hub if hub is not None else Hub()
    checkpoint()
    progress(0, None, "Resolving revision")
    with hub.open(f"/api/models/{repo}/revision/{quote(revision)}?blobs=true") as stream:
        info = read_json(stream)
    commit = info.get("sha") if isinstance(info, dict) else None
    if not isinstance(commit, str) or not COMMIT.fullmatch(commit):
        raise EngineError("Hugging Face did not provide a valid model revision.")
    source_key = f"{repo.casefold()}@{commit}:{selected or ''}"
    checkpoint()
    cached = store.receipt(source_key)
    if cached:
        return cached

    def open_file(name):
        return hub.open(f"/{repo}/resolve/{commit}/{quote(name, safe='/')}")

    entries = hub_entries(info)
    sizes = {name: item.get("size") if type(item.get("size")) is int else None
             for name, item in entries.items()}
    card = info.get("cardData")
    card = card if isinstance(card, dict) else {}
    compatibility_note = None
    with store.stage() as stage:
        directory = Path(stage)
        if MANIFEST in sizes and selected is None:
            with open_file(MANIFEST) as stream:
                metadata, hashes = package_spec(read_json(stream))
            if not hashes.keys() <= sizes.keys():
                raise EngineError("The Hugging Face model package is incomplete.")
            names = list(hashes)
        else:
            weights = sorted(name for name in sizes if Path(name).suffix.lower() in MODEL_SUFFIXES)
            preferred = next((name for name in ("model.onnx", "onnx/model.onnx") if name in weights), None)
            if selected and selected not in weights:
                raise EngineError("The requested model file was not found at this revision.")
            otherwise = selected or preferred or (weights[0] if len(weights) == 1 else None)
            if weights and otherwise is None:
                raise EngineError("Choose a Hugging Face weight file URL to select a model variant.")
            sidecars = ["config.json", "tokenizer.json", "README.md", "LICENSE", "LICENSE.txt"]
            tokenizer_size = sizes.get("tokenizer.json")
            if tokenizer_size is not None and tokenizer_size > MAX_TOKENIZER:
                sidecars.remove("tokenizer.json")
                compatibility_note = f"The tokenizer exceeds Veil's {MAX_TOKENIZER // 1024**2} MiB limit and was omitted."
            names = ([otherwise] if otherwise else []) + [name for name in sidecars if name in sizes]
            hashes = None
        if not names:
            raise EngineError("The repository has no supported model artifacts or inspectable metadata.")
        checkpoint()
        verified_hashes = hub_hashes(entries, names, hashes or {})
        files = copy_artifacts(directory, names, open_file, checkpoint, progress,
                               verified_hashes, {name: sizes[name] for name in names},
                               tokenizers=tokenizer_files(metadata) if hashes is not None else ())
        if hashes is None:
            license_name = card.get("license")
            try:
                license_name = bounded_string(license_name, "model license")
            except EngineError:
                license_name = "Unspecified"
            metadata = raw_metadata(repo, commit, license_name, directory, files, otherwise, card.get("language", ()))
            if compatibility_note:
                metadata["compatibility_note"] = compatibility_note
        metadata["source"] = {"repository": repo, "revision": commit,
                              "kind": "raw" if hashes is None else "package"}
        return store.publish(directory, metadata, files, source_key, checkpoint)


def hub_entries(info):
    siblings = info.get("siblings")
    if not isinstance(siblings, list):
        raise EngineError("Hugging Face did not provide a model file list.")
    return {entry["rfilename"]: entry for entry in siblings
            if isinstance(entry, dict) and isinstance(entry.get("rfilename"), str)}


def hub_hashes(entries, names, expected):
    hashes = dict(expected)
    for name in names:
        lfs = entries[name].get("lfs")
        if lfs is None:
            continue
        digest = lfs.get("sha256") if isinstance(lfs, dict) else None
        if not isinstance(digest, str) or not HASH.fullmatch(digest):
            raise EngineError("Hugging Face returned an invalid file checksum.")
        if name in hashes and hashes[name] != digest:
            raise EngineError("The Veil manifest and Hugging Face disagree on a file checksum.")
        hashes[name] = digest
    return hashes


def matches_file(directory, name, entry, checkpoint):
    try:
        with open_local(directory, name) as stream:
            digest = hashlib.sha256()
            remaining = entry["size"]
            while remaining:
                checkpoint()
                block = stream.read(min(1024**2, remaining))
                if not block:
                    return False
                digest.update(block)
                remaining -= len(block)
            return not stream.read(1) and digest.hexdigest() == entry["sha256"]
    except OSError:
        return False
    except EngineError:
        # A missing file is repairable. Propagate cancellation and unsafe symlink errors.
        if not (directory / name).exists() and not (directory / name).is_symlink():
            checkpoint()
            return False
        raise


def repair(store, model_id, authorized, checkpoint, progress, hub=None):
    if authorized is not True:
        raise EngineError("Authorize the model repair download first.")
    old = store.model(model_id)
    store.require_inactive(model_id)
    source = old.get("source", {})
    repo, revision = source.get("repository", ""), source.get("revision", "")
    if (not repo or not isinstance(revision, str) or not COMMIT.fullmatch(revision)
            or parse_source(repo) != (repo, "main", None)):
        raise EngineError("This model has no pinned Hugging Face source for repair.")
    checkpoint()
    progress(0, None, "Checking installed files")
    hub = hub if hub is not None else Hub()
    with hub.open(f"/api/models/{repo}/revision/{revision}?blobs=true") as stream:
        info = read_json(stream)
    if not isinstance(info, dict) or info.get("sha") != revision:
        raise EngineError("Hugging Face did not return the installed model revision.")
    entries = hub_entries(info)
    current = old["files"]
    root = store.models / model_id
    weights = [name for name in current if name.endswith(".onnx")]
    raw = source.get("kind") == "raw" or (source.get("kind") is None and not old["adapters"] and len(weights) == 1)
    names = list(current)
    if raw and weights and "tokenizer.json" in entries and "tokenizer.json" not in current:
        names.append("tokenizer.json")
    if not set(names) <= entries.keys():
        raise EngineError("The installed artifact list does not match the pinned revision.")
    sizes = {name: entries[name].get("size") if type(entries[name].get("size")) is int else None for name in names}
    for name, entry in current.items():
        if sizes[name] is not None and sizes[name] != entry["size"]:
            raise EngineError("The pinned revision disagrees with an installed file size.")
        sizes[name] = entry["size"]
    if len(names) > 32 or sum(size for size in sizes.values() if size is not None) > MAX_BUNDLE:
        raise EngineError("The repair exceeds the model installation size limit.")
    expected = hub_hashes(entries, names, {name: entry["sha256"] for name, entry in current.items()})
    missing = [name for name in names if name not in current or not matches_file(root, name, current[name], checkpoint)]
    checkpoint()

    def open_file(name):
        return hub.open(f"/{repo}/resolve/{revision}/{quote(name, safe='/')}")

    with store.stage() as stage:
        staged = Path(stage)
        restored = (copy_artifacts(staged, missing, open_file, checkpoint, progress, expected,
                                   {name: sizes[name] for name in missing}, tokenizers=tokenizer_files(old))
                    if missing else {})
        files = {**current, **restored}
        size = sum(entry["size"] for entry in files.values())
        if size > MAX_BUNDLE:
            raise EngineError("The repair exceeds the model installation size limit.")
        updated = {**old, "files": files, "size": size}
        if "tokenizer.json" in files:
            updated.pop("compatibility_note", None)
        approved = store.label_approvals(exclude_model=model_id)
        if "adapter_candidates" in old:
            updated.update(resolved_metadata(old, old["adapter_candidates"], files, approved))
        elif raw and len(weights) == 1:
            card = info.get("cardData")
            languages = card.get("language", ()) if isinstance(card, dict) else ()
            inferred = raw_metadata(old["name"], revision, old["license"],
                                    staged if "config.json" in restored else root, files, weights[0], languages)
            updated.update(inferred)
            if "adapter_candidates" in inferred:
                updated.update(resolved_metadata(old, inferred["adapter_candidates"], files, approved))
        return store.update(old, updated, checkpoint, staging=staged, staged_files=missing)
