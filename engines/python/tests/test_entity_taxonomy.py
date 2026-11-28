import hashlib
from pathlib import Path
import tempfile
import unittest

from veil_engine.components import Capability, EngineError
from veil_engine.labels import is_custom, resolve_labels
from veil_engine.model_specs import adapter_metadata
from veil_engine.model_store import Store, canonical
from veil_engine.runtime import span
from veil_engine.service import Engine


class TaxonomyTests(unittest.TestCase):
    def test_synonyms_abbreviations_and_naming_variants_resolve_to_the_same_code(self):
        labels = ["O", "B-CREDIT_CARD_CVV", "I-cvv", "S-CardSecurityCode", "B-Drivers License",
                  "B-driver-licence-number", "B-IBAN", "I-internationalBankAccountNumber",
                  "B-emailAddress", "I-e-mail", "B-ＺＩＰ code", "B-SURNAME", "B-LastName", "B-IPV6"]
        resolved, _ = resolve_labels(labels, Capability.DETECT, {})
        self.assertEqual(resolved, [
            "O", "B-CUSTOM_CREDIT_CARD_CVV", "I-CUSTOM_CREDIT_CARD_CVV", "U-CUSTOM_CREDIT_CARD_CVV",
            "B-CUSTOM_DRIVER_LICENSE", "B-CUSTOM_DRIVER_LICENSE", "B-IBAN_CODE", "I-IBAN_CODE",
            "B-EMAIL_ADDRESS", "I-EMAIL_ADDRESS", "B-POSTAL_CODE",
            "B-CUSTOM_FAMILY_NAME", "B-CUSTOM_FAMILY_NAME", "B-IP_ADDRESS",
        ])

    def test_taxonomy_keeps_different_sensitive_categories_distinct(self):
        labels = ["DATE", "DATE_OF_BIRTH", "TIME", "PIN", "API_KEY", "PASSWORD", "SECRET",
                  "CREDIT_DEBIT_CARD", "CVV", "PASSPORT", "SSN", "NATIONAL_ID", "GOVERNMENT_ID",
                  "ACCOUNT_ID", "ACCOUNT_NUMBER", "BANK_ACCOUNT", "IBAN", "CITY", "COUNTRY",
                  "MEDICAL_RECORD_NUMBER", "MEDICAL_ID", "PERSON", "GIVEN_NAME", "SURNAME"]
        resolved, _ = resolve_labels(labels, Capability.CLASSIFY, {})
        self.assertNotIn(None, resolved)
        self.assertEqual(len(set(resolved)), len(labels))

    def test_unknowns_get_stable_suggestions_without_resolving_or_ignoring_them(self):
        sample = ["O", "B-ContactBadge", "I-ContactBadge", "B-CONTACT_BADGE", "B-名札", "B-LABEL_1",
                  "B-VERY_LONG_UNFAMILIAR_ENTITY_LABEL_ALPHA", "B-VERY_LONG_UNFAMILIAR_ENTITY_LABEL_OMEGA"]
        resolved, groups = resolve_labels(sample, Capability.DETECT, {}, scope="first-model")
        self.assertEqual(resolved, ["O", *[None] * (len(sample) - 1)])
        _, reversed_groups = resolve_labels(list(reversed(sample)), Capability.DETECT, {}, scope="first-model")
        _, other_groups = resolve_labels(sample, Capability.DETECT, {}, scope="second-model")
        suggestions = {group["label"]: group["suggested"] for group in groups}
        self.assertEqual(suggestions, {group["label"]: group["suggested"] for group in reversed_groups})
        self.assertEqual(suggestions["CONTACTBADGE"], suggestions["CONTACT_BADGE"])
        self.assertEqual(len(set(suggestions.values())), len(groups) - 1)
        self.assertTrue(all(is_custom(code) for code in suggestions.values()))
        self.assertNotEqual(suggestions["LABEL_1"], {group["label"]: group["suggested"] for group in other_groups}["LABEL_1"])
        self.assertIsNone(next(group for group in groups if group["label"] == "LABEL_1")["reuse_key"])


class ApprovalTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.store = Store(temp.name)
        self.addCleanup(self.store.db.close)
        self.engine = Engine(self.store)

    def install(self, name, labels, *, author_choices=None):
        candidate = {"adapter": "onnx-token-v1", "model": "fixture.onnx", "tokenizer": "tokenizer.json",
                     "labels": labels, "languages": ["en"]}
        if author_choices:
            candidate["label_map"] = author_choices
        with self.store.stage() as directory:
            path = Path(directory)
            files = {}
            for filename, data in [("fixture.onnx", b"Fictional graph"), ("tokenizer.json", b"{}")]:
                (path / filename).write_bytes(data)
                files[filename] = {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()}
            return self.store.publish(path, {"name": name, "version": "fixture", "license": "CC0-1.0",
                                            **adapter_metadata([candidate], files)}, files, None, lambda: None)

    def configure(self, model, choices):
        return self.engine.dispatch({"action": "model_configure", "id": model["id"],
                                     "mappings": {"onnx-token-v1": choices}})

    def groups(self, model):
        response = self.engine.dispatch({"action": "model_configuration", "id": model["id"]})
        return {group["label"]: group for item in response["adapters"] for group in item["groups"]}

    def test_positive_approvals_reuse_synonyms_and_persist_without_model_or_network(self):
        first = self.install("first", ["O", "B-CVV", "B-PrivateBadge", "B-GATE_CODE", "B-LABEL_0"])
        self.configure(first, {"CVV": "CUSTOM_PAYMENT_VERIFIER", "PRIVATEBADGE": "CUSTOM_STAFF_BADGE",
                               "GATE_CODE": "@ignore", "LABEL_0": "CUSTOM_GATE_CODE"})
        self.store.remove(first["id"])
        reopened = Store(self.store.root)
        self.addCleanup(reopened.db.close)
        self.assertEqual(reopened.label_approvals(), {
            "category:CUSTOM_CREDIT_CARD_CVV": "CUSTOM_PAYMENT_VERIFIER",
            "label:PRIVATE_BADGE": "CUSTOM_STAFF_BADGE",
        })
        second = self.install("second", ["O", "B-Credit Card CVV", "I-PRIVATE_BADGE"])
        self.assertEqual(second["adapters"][0]["labels"],
                         ["O", "B-CUSTOM_PAYMENT_VERIFIER", "I-CUSTOM_STAFF_BADGE"])
        draft = self.install("third", ["O", "B-PRIVATE_BADGE", "B-GATE_CODE", "B-LABEL_0"])
        groups = self.groups(draft)
        self.assertEqual(groups["PRIVATE_BADGE"]["reason"], "approved")
        self.assertIsNone(groups["GATE_CODE"]["target"])
        self.assertIsNone(groups["LABEL_0"]["target"])
        self.assertFalse(draft["adapters"])

    def test_conflict_requires_a_local_choice_and_updates_keep_old_runtime_snapshot(self):
        first = self.install("first", ["O", "B-CASE_ID"])
        self.configure(first, {"CASE_ID": "CUSTOM_FIRST_CASE"})
        second = self.install("second", ["O", "B-CaseID"])
        self.assertEqual(second["adapters"][0]["labels"][1], "B-CUSTOM_FIRST_CASE")
        self.configure(second, {"CASEID": "CUSTOM_SECOND_CASE"})
        draft = self.install("third", ["O", "B-CASE_ID"])
        group = self.groups(draft)["CASE_ID"]
        self.assertEqual((group["target"], group["reason"], group["suggested"]),
                         (None, "conflict", "CUSTOM_CASE_ID"))
        self.assertFalse(draft["adapters"])
        self.assertEqual(self.store.model(first["id"])["adapters"][0]["labels"][1], "B-CUSTOM_FIRST_CASE")

    def test_two_models_with_equivalent_labels_fuse_as_one_category(self):
        first = self.install("first", ["O", "B-CVV"])
        second = self.install("second", ["O", "B-CreditCardCVV"])

        class FixtureModel:
            def __init__(self, category):
                self.category = category

            def detect(self, request):
                return [span(request.text, 7, 10, self.category, 0.9)]

        detectors = []
        for model in (first, second):
            label = model["adapters"][0]["labels"][1]
            self.engine.sessions.loaded[model["id"]] = FixtureModel(label.removeprefix("B-"))
            detectors.append({"component": "model:" + model["id"], "capability": "span_detection"})
        profile = self.engine.profiles.save({
            "schema": 1, "name": "Two offline classifiers", "policy": "static", "budget": {},
            "legacy_nlp": False, "languages": ["en"],
            "steps": [*detectors, {"component": "exact-span", "capability": "detection_fusion"},
                      {"component": "veil-ranking", "capability": "entity_ranking"}],
        })
        self.engine.profiles.activate(profile["id"])
        result = self.engine.dispatch({"action": "detect", "text": "😀 CVV: 483", "language": "en",
                                       "entities": ["CUSTOM_CREDIT_CARD_CVV"]})
        self.assertEqual(result["detections"], [span("😀 CVV: 483", 7, 10, "CUSTOM_CREDIT_CARD_CVV", 0.9)])
        self.assertEqual({metric["component"] for metric in self.engine.last_run["metrics"]
                          if metric["component"].startswith("model:") and metric["invocations"]},
                         {step["component"] for step in detectors})

    def test_mapping_reset_previews_changes_and_restores_taxonomy_without_reimport(self):
        prior = self.install("other installation", ["O", "B-CREDIT_CARD_CVV"])
        self.configure(prior, {"CREDIT_CARD_CVV": "CUSTOM_PRIOR_VERIFIER"})
        model = self.install("legacy mappings", ["O", "B-CVV", "B-CREDIT_DEBIT_CARD", "B-DOB", "B-DATE", "B-ACCESS_ZONE"])
        self.configure(model, {"CVV": "CUSTOM_OLD_CODE", "CREDIT_DEBIT_CARD": "CUSTOM_CARD",
                               "DOB": "DATE_TIME", "ACCESS_ZONE": "@ignore"})
        model_id = model["id"]
        saved = self.store.model(model_id)
        profile = self.engine.profiles.save({
            "schema": 1, "name": "Saved detection profile", "policy": "static", "budget": {},
            "legacy_nlp": False, "languages": ["en"],
            "steps": [{"component": component, "capability": task} for component, task in [
                ("model:" + model_id, "span_detection"), ("exact-span", "detection_fusion"),
                ("veil-ranking", "entity_ranking")]],
        })
        profile_before = self.engine.profiles.get(profile["id"])
        directory = self.store.models / model_id
        files_before = {path.name: (path.stat().st_ino, path.stat().st_mtime_ns, path.read_bytes())
                        for path in directory.iterdir()}
        preview = self.engine.dispatch({"action": "model_label_reset_preview", "id": model_id})
        self.assertTrue(preview["applicable"])
        self.assertEqual({row["label"]: (row["current"], row["recommended"]) for row in preview["changes"]}, {
            "CVV": ("CUSTOM_OLD_CODE", "CUSTOM_CREDIT_CARD_CVV"),
            "CREDIT_DEBIT_CARD": ("CUSTOM_CARD", "CREDIT_CARD"),
            "DOB": ("DATE_TIME", "CUSTOM_DATE_OF_BIRTH"),
            "ACCESS_ZONE": ("@ignore", "CUSTOM_ACCESS_ZONE"),
        })
        self.assertEqual(self.store.model(model_id), saved)

        confirm = {"action": "model_label_reset", "id": model_id, "reset_token": preview["token"]}
        self.engine.sessions.loaded[model_id] = object()
        self.engine.profiles.activate(profile["id"])
        with self.assertRaisesRegex(EngineError, "active profile"):
            self.engine.dispatch(confirm)
        self.assertEqual(self.store.model(model_id), saved)
        self.engine.profiles.activate(None)
        self.configure(model, {"CVV": "CUSTOM_ANOTHER_CODE"})
        with self.assertRaisesRegex(EngineError, "Preview the reset again"):
            self.engine.dispatch(confirm)
        latest = self.engine.dispatch({"action": "model_label_reset_preview", "id": model_id})
        self.engine.sessions.loaded[model_id] = object()
        self.engine.dispatch({**confirm, "reset_token": latest["token"]})
        updated = self.store.model(model_id)
        self.assertEqual(updated["adapters"][0]["labels"],
                         ["O", "B-CUSTOM_CREDIT_CARD_CVV", "B-CREDIT_CARD",
                          "B-CUSTOM_DATE_OF_BIRTH", "B-DATE_TIME", "B-CUSTOM_ACCESS_ZONE"])
        self.assertNotIn(model_id, self.engine.sessions.loaded)
        self.assertEqual(updated["files"], saved["files"])
        self.assertEqual(self.engine.profiles.get(profile["id"]), profile_before)
        self.assertEqual({path.name: (path.stat().st_ino, path.stat().st_mtime_ns, path.read_bytes())
                          for path in directory.iterdir()}, files_before)
        self.assertFalse(self.engine.dispatch({"action": "model_label_reset_preview", "id": model_id})["applicable"])

    def test_mapping_reset_can_publish_a_legacy_draft_with_newly_recognized_labels(self):
        draft = self.install("unconfigured", ["O", "B-CVV"])
        draft["adapters"] = []
        self.store.db.execute("UPDATE models SET data = ? WHERE id = ?", (canonical(draft), draft["id"]))
        preview = self.engine.dispatch({"action": "model_label_reset_preview", "id": draft["id"]})
        self.assertTrue(preview["applicable"])
        self.assertEqual(preview["changes"], [])
        self.engine.dispatch({"action": "model_label_reset", "id": draft["id"], "reset_token": preview["token"]})
        self.assertEqual(self.store.model(draft["id"])["adapters"][0]["labels"], ["O", "B-CUSTOM_CREDIT_CARD_CVV"])

    def test_old_install_and_profile_survive_additive_migration(self):
        old = self.install("legacy", ["O", "B-DOB", "B-PrivateBadge"])
        self.configure(old, {"PRIVATEBADGE": "CUSTOM_STAFF_BADGE"})
        old = self.store.model(old["id"])
        old["adapters"][0]["labels"][1] = "B-DATE_TIME"
        self.store.db.execute("UPDATE models SET data = ? WHERE id = ?", (canonical(old), old["id"]))
        pristine = self.install("publisher choices", ["O", "B-PrintedIndex"],
                                author_choices={"PRINTEDINDEX": "CUSTOM_PUBLISHER_CODE"})
        self.assertTrue(pristine["adapters"])
        profile = self.engine.profiles.save({
            "schema": 1, "name": "Existing profile", "policy": "static", "budget": {},
            "legacy_nlp": False, "languages": ["en"],
            "steps": [{"component": component, "capability": task} for component, task in [
                ("model:" + old["id"], "span_detection"), ("exact-span", "detection_fusion"),
                ("veil-ranking", "entity_ranking")]],
        })
        self.engine.sessions.loaded[old["id"]] = object()
        self.engine.profiles.activate(profile["id"])
        self.store.db.executescript("DROP TABLE label_approvals; PRAGMA user_version = 1;")
        reopened = Store(self.store.root)
        self.addCleanup(reopened.db.close)
        self.store = reopened
        self.engine = Engine(reopened)
        self.assertEqual(reopened.model(old["id"]), old)
        self.assertEqual(self.engine.profiles.active_id(), profile["id"])
        self.assertEqual(self.engine.profiles.get(profile["id"]), {key: value for key, value in profile.items() if key != "id"})
        self.assertEqual(reopened.label_approvals(), {"label:PRIVATE_BADGE": "CUSTOM_STAFF_BADGE"})
        dob = self.groups(old)["DOB"]
        self.assertEqual((dob["target"], dob["suggested"]), ("DATE_TIME", "CUSTOM_DATE_OF_BIRTH"))
        decisions = {"PRIVATEBADGE": "CUSTOM_STAFF_BADGE"}
        with self.assertRaisesRegex(EngineError, "active profile"):
            self.configure(old, decisions)
        self.engine.profiles.activate(None)
        self.configure(old, decisions)
        self.assertEqual(self.store.model(old["id"])["adapters"][0]["labels"][1], "B-DATE_TIME")
        decisions["DOB"] = dob["suggested"]
        self.configure(old, decisions)
        self.assertEqual(self.store.model(old["id"])["adapters"][0]["labels"][1], "B-CUSTOM_DATE_OF_BIRTH")
