"""Hugging Face download policy and verification without a live network."""
import hashlib
import io
import json
import tempfile
import unittest
from urllib.request import Request

from veil_engine.components import EngineError
from veil_engine.model_download import ModelRedirects, download
from veil_engine.model_import import MAX_TOKENIZER
from veil_engine.model_specs import MANIFEST
from veil_engine.model_store import MAX_FILE, Store

REPO = "example/fictional-ner"
COMMIT = "b" * 40
WEIGHTS = "onnx/model_quantized.onnx"
DATA = b"fictional model weights"
DIGEST = hashlib.sha256(DATA).hexdigest()


class RedirectTests(unittest.TestCase):
    def setUp(self):
        self.handler = ModelRedirects()
        self.request = Request(f"https://huggingface.co/{REPO}/resolve/{COMMIT}/{WEIGHTS}")

    def follow(self, destination):
        return self.handler.redirect_request(self.request, None, 302, "Found", {}, destination)

    def test_trusted_cdn_lfs_xet_and_hub_redirects_keep_the_signed_url(self):
        for host in (
            "huggingface.co", "us.aws.cdn.hf.co", "europe-west4.us.gcp.cdn.hf.co",
            "CDN-LFS-EU-1.HF.CO:443", "cdn-lfs.huggingface.co", "cas-bridge.xethub.hf.co",
            "transfer.xethub-eu.hf.co",
        ):
            with self.subTest(host=host):
                url = f"https://{host}/artifact?signature=fixture%2B%2F%3D"
                redirected = self.follow(url)
                self.assertEqual(redirected.full_url, url)
                self.assertEqual(redirected.get_method(), "GET")

    def test_redirects_recheck_authority_and_scheme(self):
        self.request = self.follow("https://us.aws.cdn.hf.co/artifact")
        for url in (
            "http://us.aws.cdn.hf.co/file",
            "https://us.aws.cdn.hf.co:444/file",
            "https://other.aws.cdn.hf.co/file",
            "https://us.aws.cdn.hf.co.example.test/file",
            "https://us.aws.cdn.hf.co./file",
            "https://example.test@us.aws.cdn.hf.co/file",
            "https://us.aws.cdn.hf.co@example.test/file",
            "https://us%2eaws.cdn.hf.co/file",
            "https://anyone.hf.co/file",
            "https://127.0.0.1/file",
            "https://127.0.0.1:443/file",
            "https://s3.us-east-1.amazonaws.com/other-bucket/file",
        ):
            with self.subTest(url=url), self.assertRaisesRegex(EngineError, "unsupported download destination"):
                self.follow(url)


class FakeHub:
    def __init__(self, entries, files):
        self.requested = []
        self.files = {f"/{REPO}/resolve/{COMMIT}/{name}": value for name, value in files.items()}
        self.files[f"/api/models/{REPO}/revision/main?blobs=true"] = json.dumps(
            {"sha": COMMIT, "siblings": entries, "cardData": {"language": ["en"]}}
        ).encode()

    def open(self, path):
        self.requested.append(path)
        return io.BytesIO(self.files[path])


class IntegrityTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.store = Store(directory.name)
        self.addCleanup(self.store.db.close)

    def import_model(self, hub, selected=True):
        url = f"https://huggingface.co/{REPO}/resolve/main/{WEIGHTS}" if selected else REPO
        return download(self.store, url, True, lambda: None, lambda *_: None, hub)

    def test_pinned_lfs_download_survives_oversized_optional_tokenizer(self):
        entries = [
            {"rfilename": WEIGHTS, "size": len(DATA), "lfs": {"sha256": DIGEST}},
            {"rfilename": "tokenizer.json", "size": MAX_TOKENIZER + 1},
        ]
        hub = FakeHub(entries, {WEIGHTS: DATA})
        item = self.import_model(hub)
        self.assertEqual(hub.requested[-1], f"/{REPO}/resolve/{COMMIT}/{WEIGHTS}")
        self.assertEqual(item["files"], {WEIGHTS: {"sha256": DIGEST, "size": len(DATA)}})
        self.assertEqual(item["source"]["revision"], COMMIT)
        self.assertEqual(item["adapters"], [])
        self.assertIn(f"{MAX_TOKENIZER // 1024**2} MiB", item["compatibility_note"])

    def test_bad_lfs_digest_rejects_same_size_artifact_without_publishing(self):
        entries = [{"rfilename": WEIGHTS, "size": len(DATA), "lfs": {"sha256": "0" * 64}}]
        hub = FakeHub(entries, {WEIGHTS: DATA})
        with self.assertRaisesRegex(EngineError, "incorrect checksum"):
            self.import_model(hub)
        self.assertEqual(self.store.installed(), [])
        self.assertFalse(list(self.store.models.iterdir()))

    def test_manifest_and_lfs_checksums_must_agree(self):
        manifest = {"schema": 1, "name": "Synthetic model", "version": "1", "license": "CC0-1.0",
                    "files": {WEIGHTS: "0" * 64}}
        entries = [{"rfilename": WEIGHTS, "size": len(DATA), "lfs": {"sha256": DIGEST}},
                   {"rfilename": MANIFEST}]
        hub = FakeHub(entries, {MANIFEST: json.dumps(manifest).encode()})
        with self.assertRaisesRegex(EngineError, "disagree"):
            self.import_model(hub, selected=False)
        self.assertEqual(hub.requested[-1], f"/{REPO}/resolve/{COMMIT}/{MANIFEST}")
        self.assertEqual(self.store.installed(), [])

    def test_required_model_and_manifest_tokenizer_limits_still_apply(self):
        hub = FakeHub([{"rfilename": WEIGHTS, "size": MAX_FILE + 1}], {})
        with self.assertRaisesRegex(EngineError, "size limit"):
            self.import_model(hub)
        self.assertEqual(len(hub.requested), 1)
        manifest = {"schema": 1, "name": "Synthetic model", "version": "1", "license": "CC0-1.0",
                    "files": {"tokenizer.json": "0" * 64}}
        entries = [{"rfilename": "tokenizer.json", "size": MAX_TOKENIZER + 1}, {"rfilename": MANIFEST}]
        hub = FakeHub(entries, {MANIFEST: json.dumps(manifest).encode()})
        with self.assertRaisesRegex(EngineError, "tokenizer exceeds"):
            self.import_model(hub, selected=False)
        self.assertEqual(len(hub.requested), 2)
        self.assertEqual(self.store.installed(), [])
