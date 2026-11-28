"""Shared, immutable model artifacts and transactional engine settings."""
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import tempfile

from veil_engine.components import EngineError
from veil_engine.model_configuration import approved_targets
from veil_engine.model_specs import HASH, adapter_metadata, artifact_path

MAX_FILE = 512 * 1024**2
MAX_BUNDLE = 1024**3
BLOCK_SIZE = 1024**2


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=True, allow_nan=False, separators=(",", ":"))


class Store:
    def __init__(self, root):
        self.root = Path(root)
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.models = self.root / "models"
        self.models.mkdir(mode=0o700, exist_ok=True)
        self.db = sqlite3.connect(self.root / "engine.sqlite3", isolation_level=None, timeout=10)
        self.db.execute("PRAGMA foreign_keys = ON")
        self.db.execute("PRAGMA trusted_schema = OFF")
        version = self.db.execute("PRAGMA user_version").fetchone()[0]
        if version not in (0, 1, 2):
            raise EngineError("This model library uses a newer database schema.")
        if version < 2:
            self.db.executescript("""
                BEGIN IMMEDIATE;
                CREATE TABLE IF NOT EXISTS models (id TEXT PRIMARY KEY, data TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS sources (
                    key TEXT PRIMARY KEY, model_id TEXT NOT NULL REFERENCES models ON DELETE CASCADE
                );
                CREATE TABLE IF NOT EXISTS profiles (id TEXT PRIMARY KEY, data TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS active (
                    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                    profile_id TEXT NOT NULL REFERENCES profiles
                );
                CREATE TABLE IF NOT EXISTS label_approvals (
                    model_id TEXT NOT NULL,
                    label_key TEXT NOT NULL,
                    target TEXT NOT NULL,
                    PRIMARY KEY (model_id, label_key, target)
                );
            """)
            try:
                if self.db.execute("PRAGMA user_version").fetchone()[0] < 2:
                    for model in self.installed():
                        # A pristine imported manifest contains publisher choices, not local approvals.
                        if model.get("generation", 0):
                            self.record_label_approvals(model["id"], approved_targets(model))
                    self.db.execute("PRAGMA user_version = 2")
                self.db.commit()
            except BaseException:
                self.db.rollback()
                raise

    @contextmanager
    def write(self):
        self.db.execute("BEGIN IMMEDIATE")
        try:
            yield
            self.db.commit()
        except BaseException:
            self.db.rollback()
            raise

    def model(self, model_id):
        if isinstance(model_id, str) and HASH.fullmatch(model_id):
            row = self.db.execute("SELECT data FROM models WHERE id = ?", (model_id,)).fetchone()
            if row:
                return json.loads(row[0])
        raise EngineError("Select an installed model.")

    def contains(self, model_id):
        return self.db.execute("SELECT 1 FROM models WHERE id = ?", (model_id,)).fetchone() is not None

    def installed(self):
        return [json.loads(row[0]) for row in self.db.execute("SELECT data FROM models ORDER BY id")]

    def receipt(self, source_key):
        row = self.db.execute("SELECT model_id FROM sources WHERE key = ?", (source_key,)).fetchone()
        return self.model(row[0]) if row else None

    def label_approvals(self, *, exclude_model=None):
        rows = self.db.execute("""
            SELECT label_key, MIN(target), COUNT(DISTINCT target)
            FROM label_approvals WHERE (? IS NULL OR model_id != ?) GROUP BY label_key
        """, (exclude_model, exclude_model))
        return {key: target if count == 1 else None for key, target, count in rows}

    def record_label_approvals(self, model_id, approvals):
        self.db.execute("DELETE FROM label_approvals WHERE model_id = ?", (model_id,))
        self.db.executemany("INSERT INTO label_approvals VALUES (?, ?, ?)",
                            ((model_id, key, target) for key, target in approvals))

    def stage(self):
        return tempfile.TemporaryDirectory(prefix=".install-", dir=self.models)

    def publish(self, directory, metadata, files, source_key, checkpoint):
        identity = {key: value for key, value in metadata.items() if key != "source"}
        identity["files"] = files
        model_id = hashlib.sha256(canonical(identity).encode()).hexdigest()
        model = {**metadata, "id": model_id, "files": files,
                 "size": sum(item["size"] for item in files.values())}
        target = self.models / model_id
        with self.write():
            checkpoint()
            row = self.db.execute("SELECT data FROM models WHERE id = ?", (model_id,)).fetchone()
            if row:
                model = json.loads(row[0])
            else:
                # A crash between rename and commit can only leave an unreferenced directory.
                if target.exists():
                    shutil.rmtree(target)
                if os.name == "posix":
                    for parent, _, _ in os.walk(directory, topdown=False):
                        sync_directory(parent)
                checkpoint()
                directory.rename(target)
                if os.name == "posix":
                    sync_directory(self.models)
                if "adapter_candidates" in model:
                    model.update(adapter_metadata(model["adapter_candidates"], files, approved=self.label_approvals()))
                self.db.execute("INSERT INTO models VALUES (?, ?)", (model_id, canonical(model)))
            if source_key:
                self.db.execute("INSERT OR REPLACE INTO sources VALUES (?, ?)", (source_key, model_id))
        return model

    def in_use(self, model_id):
        row = self.db.execute("""
            SELECT profiles.data FROM profiles JOIN active ON profiles.id = active.profile_id
        """).fetchone()
        return bool(row and any(step["component"] == f"model:{model_id}" for step in json.loads(row[0])["steps"]))

    def require_inactive(self, model_id):
        if self.in_use(model_id):
            raise EngineError("Switch profiles before changing a model required by the active profile.")

    def update(self, previous, updated, checkpoint, *, staging=None, staged_files=(), approvals=None):
        model_id = previous["id"]
        updated = {**updated, "generation": previous.get("generation", 0) + 1}
        with self.write():
            self.require_inactive(model_id)
            if self.model(model_id) != previous:
                raise EngineError("The installed model changed. Refresh the library and try again.")
            checkpoint()
            target = self.models / model_id
            if target.is_symlink():
                raise EngineError("The installed model directory is unsafe.")
            target.mkdir(mode=0o700, exist_ok=True)
            for name in staged_files:
                destination = target / artifact_path(name)
                parent = destination.parent
                while parent != target:
                    if parent.is_symlink():
                        raise EngineError("The installed model directory is unsafe.")
                    parent = parent.parent
                destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                # Replacements have the original verified digest. A new file becomes visible in the catalog at commit.
                os.replace(staging / name, destination)
                if os.name == "posix":
                    parent = destination.parent
                    while parent != self.models:
                        sync_directory(parent)
                        parent = parent.parent
            self.db.execute("UPDATE models SET data = ? WHERE id = ?", (canonical(updated), model_id))
            if approvals is not None:
                self.record_label_approvals(model_id, approvals)
        return updated

    def remove(self, model_id):
        with self.write():
            self.model(model_id)
            if self.in_use(model_id):
                raise EngineError("Switch or edit the active profile before removing this model.")
            self.db.execute("DELETE FROM models WHERE id = ?", (model_id,))
        with self.write():
            # Another process may have reinstalled identical files after the deletion.
            if not self.contains(model_id):
                try:
                    shutil.rmtree(self.models / model_id)
                except FileNotFoundError:
                    pass
                except OSError:
                    raise EngineError("The model was removed from the library, but its local files could not be deleted.") from None


def sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
