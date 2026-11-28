"""Private line-delimited IPC for the desktop. Only the download entry point has network access."""
import json
from pathlib import Path
import sys

from veil_engine.components import EngineError, algorithms
from veil_engine.entity_taxonomy import CATEGORIES
from veil_engine.model_configuration import (can_reset_mapping, candidates_for, configure,
                                              describe_configuration, preview_mapping_reset, reset_mapping)
from veil_engine.model_import import install_local, read_json
from veil_engine.model_store import Store
from veil_engine.pipeline import CORE, CORE_NAME, run_detection
from veil_engine.profiles import Profiles, custom_categories
from veil_engine.runtime import ModelSessions

MAX_REQUEST = 5_000_000
MAX_RESPONSE = 16_000_000


def offline():
    def reject_network(event, args):
        if event in {"socket.connect", "socket.getaddrinfo", "socket.gethostbyname",
                     "socket.gethostbyaddr", "socket.getnameinfo", "socket.sendto"}:
            raise EngineError("Network access is disabled during local inference.")

    sys.addaudithook(reject_network)


class Engine:
    def __init__(self, store, sessions=None):
        self.store = store
        self.sessions = sessions if sessions is not None else ModelSessions()
        self.registry = algorithms()
        self.profiles = Profiles(store, self.registry, self.sessions)
        self.last_run = None

    def dispatch(self, request):
        match request["action"]:
            case "status":
                core = {"id": CORE.component, "name": CORE_NAME, "capabilities": [CORE.capability], "mandatory": True}
                active = self.profiles.active_id()
                profile = self.profiles.get(active) if active else None
                models = []
                for model in self.store.installed():
                    visible = {key: value for key, value in model.items() if key != "adapter_candidates"}
                    models.append({**visible, "loaded": self.sessions.is_loaded(model),
                                   "configurable": any("labels" in spec for spec in candidates_for(model))})
                return {"models": models, "profiles": self.profiles.listing(), "active_profile": active,
                        "custom_categories": sorted(custom_categories(profile, self.store)),
                        "algorithms": [core, *(item.describe() for item in self.registry.components.values())],
                        "last_run": self.last_run}
            case "detect":
                profile_id = self.profiles.active_id()
                metrics = []
                self.last_run = {"profile_id": profile_id, "complete": False, "metrics": metrics}
                profile = self.profiles.validate(self.profiles.get(profile_id)) if profile_id else None
                cancel = Path(request["cancel_path"]).exists if request.get("cancel_path") else lambda: False
                result = run_detection(request["text"], request.get("entities"), request.get("language", "en"),
                                       request.get("rules"), request.get("options"),
                                       profile=profile, store=self.store, sessions=self.sessions,
                                       metrics=metrics, cancelled=cancel, registry=self.registry)
                self.last_run["complete"] = True
                return result
            case "model_import":
                return install_local(self.store, request["path"])
            case "model_configuration":
                model_id = request["id"]
                model = self.store.model(model_id)
                return {"adapters": describe_configuration(model, self.store.label_approvals(exclude_model=model_id)),
                        "categories": sorted(CATEGORIES), "can_reset": can_reset_mapping(model)}
            case "model_configure":
                updated = configure(self.store, request["id"], request.get("mappings"))
                self.sessions.unload(updated["id"])
                return describe_configuration(updated, self.store.label_approvals(exclude_model=updated["id"]))
            case "model_label_reset_preview":
                return preview_mapping_reset(self.store, request["id"])
            case "model_label_reset":
                updated = reset_mapping(self.store, request["id"], request.get("reset_token"))
                self.sessions.unload(updated["id"])
            case "model_load":
                self.sessions.load(self.store, request["id"])
            case "model_unload":
                self.store.model(request["id"])
                self.sessions.unload(request["id"])
            case "model_remove":
                model_id = request["id"]
                try:
                    self.store.remove(model_id)
                finally:
                    # The catalog can be removed even when disk cleanup fails.
                    if isinstance(model_id, str) and not self.store.contains(model_id):
                        self.sessions.unload(model_id)
            case "profile_save":
                return self.profiles.save(request["profile"], request.get("id"))
            case "profile_activate":
                return self.profiles.activate(request["id"])
            case "profile_delete":
                self.profiles.delete(request["id"])
            case "profile_export":
                return self.profiles.get(request["id"])
            case "profile_import":
                try:
                    path = Path(request["path"])
                    if path.is_symlink() or not path.is_file():
                        raise EngineError("Choose a local profile JSON file.")
                    with path.open("rb") as stream:
                        profile = read_json(stream)
                except OSError:
                    raise EngineError("Could not open the selected profile.") from None
                return self.profiles.save(profile)
            case _:
                raise EngineError("Unsupported local engine action.")


def respond(payload):
    frame = json.dumps(payload, allow_nan=False, ensure_ascii=True, separators=(",", ":"))
    if len(frame) >= MAX_RESPONSE:
        frame = '{"error":"The local engine response exceeded the size limit."}'
    print(frame, flush=True)


def request_from_stdin():
    line = sys.stdin.buffer.readline(MAX_REQUEST + 1)
    if len(line) > MAX_REQUEST:
        raise EngineError("The local engine request exceeded the size limit.")
    return json.loads(line) if line else None


def serve(root):
    offline()
    try:
        engine = Engine(Store(root))
    except EngineError as error:
        respond({"error": str(error)})
        return
    except Exception:
        respond({"error": "Could not open the local model and profile library."})
        return
    while True:
        try:
            request = request_from_stdin()
        except Exception:
            respond({"error": "Invalid local engine request."})
            return
        if request is None:
            return
        try:
            respond({"result": engine.dispatch(request)})
        except EngineError as error:
            respond({"error": str(error)})
        except Exception:
            respond({"error": "The local engine could not complete the requested action."})
        finally:
            del request


def download_job(root):
    from veil_engine.model_download import download, repair

    try:
        request = request_from_stdin()
        cancel = Path(request["cancel_path"])

        def checkpoint():
            if cancel.exists():
                raise EngineError("Model download cancelled.")

        def progress(received, total, file):
            respond({"progress": {"received": received, "total": total, "file": file}})

        store = Store(root)
        if request.get("model_id") is not None:
            result = repair(store, request["model_id"], request["authorized"], checkpoint, progress)
        else:
            result = download(store, request["source"], request["authorized"], checkpoint, progress)
        respond({"result": result["id"]})
    except EngineError as error:
        respond({"error": str(error)})
    except Exception:
        respond({"error": "The model download could not complete. Check the connection and local storage."})


if __name__ == "__main__":
    mode, root = sys.argv[1:]
    if mode == "serve":
        serve(root)
    elif mode == "download":
        download_job(root)
