"""Inspect and update local label decisions while preserving an installation's ID."""
import hashlib
import json

from veil_engine.components import Capability, EngineError
from veil_engine.labels import IGNORE, group_label, resolve_labels
from veil_engine.model_specs import ADAPTER_TASKS, adapter_metadata


def candidates_for(model):
    # Older installations stored only the resolved runtime descriptors.
    if "adapter_candidates" in model:
        return model["adapter_candidates"]
    return [{key: value for key, value in adapter.items() if key != "capability"}
            for adapter in model["adapters"]]


def installed_defaults(model, candidate):
    """Keep implicit decisions frozen in the installed runtime snapshot."""
    adapter = candidate["adapter"]
    previous = next((spec["labels"] for spec in model["adapters"] if spec["adapter"] == adapter), ())
    if len(previous) != len(candidate["labels"]):
        return {}
    token = ADAPTER_TASKS[adapter] == Capability.DETECT
    overrides = candidate.get("label_map", {})
    defaults = {}
    for native, resolved in zip(candidate["labels"], previous, strict=True):
        _, group = group_label(native, token)
        if group != "O" and group not in overrides:
            defaults[group] = IGNORE if resolved == "O" else group_label(resolved, token)[1]
    return defaults


def describe_configuration(model, approved=None):
    choices = []
    for candidate in candidates_for(model):
        adapter = candidate["adapter"]
        if adapter not in ADAPTER_TASKS:
            continue
        overrides = candidate.get("label_map", {})
        labels, groups = resolve_labels(
            candidate["labels"], ADAPTER_TASKS[adapter], overrides,
            approved=approved, defaults=installed_defaults(model, candidate), scope=model["id"],
        )
        choices.append({"adapter": adapter,
                        "groups": [{key: value for key, value in group.items() if key != "reuse_key"}
                                   for group in groups],
                        "overrides": overrides,
                        "unresolved": sum(group["target"] is None for group in groups),
                        "ignored": sum(group["target"] == IGNORE for group in groups),
                        "missing": [candidate[role] for role in ("model", "tokenizer")
                                    if candidate[role] not in model["files"]],
                        "sensitive": any(label not in (None, "O") for label in labels)})
    return choices


def approved_targets(model):
    decisions = set()
    for candidate in candidates_for(model):
        task = ADAPTER_TASKS.get(candidate["adapter"])
        overrides = candidate.get("label_map", {})
        if task and overrides:
            _, groups = resolve_labels(candidate["labels"], task, overrides)
            for group in groups:
                target = overrides.get(group["label"])
                if target and target != IGNORE and group["reuse_key"] is not None:
                    decisions.add((group["reuse_key"], target))
    return sorted(decisions)


def resolved_metadata(model, candidates, files, approved=None):
    defaults = {candidate["adapter"]: installed_defaults(model, candidate)
                for candidate in candidates_for(model) if candidate["adapter"] in ADAPTER_TASKS}
    return adapter_metadata(candidates, files, approved=approved, defaults=defaults)


def can_reset_mapping(model):
    return any(candidate["adapter"] in ADAPTER_TASKS for candidate in model.get("adapter_candidates", ()))


def _recommended_reset(store, model):
    if not can_reset_mapping(model):
        raise EngineError("This installation lacks the original model labels needed for a reset.")
    current = {entry["adapter"]: entry for entry in
               describe_configuration(model, store.label_approvals(exclude_model=model["id"]))}
    mappings, changes = {}, []
    ready = {adapter["adapter"] for adapter in model["adapters"]}
    publishable = False
    for candidate in model["adapter_candidates"]:
        adapter = candidate["adapter"]
        if adapter not in current:
            continue
        saved = current[adapter]
        # Start from the taxonomy and native labels, including for previously ignored groups.
        _, groups = resolve_labels(candidate["labels"], ADAPTER_TASKS[adapter], {}, scope=model["id"])
        recommended = {group["label"]: group["suggested"] for group in groups}
        mappings[adapter] = recommended
        for group in saved["groups"]:
            label, target = group["label"], group["target"]
            if target != recommended[label]:
                changes.append({"adapter": adapter, "label": label, "current": target,
                                "recommended": recommended[label]})
        if recommended and adapter not in ready and not saved["missing"]:
            publishable = True

    token = hashlib.sha256(json.dumps(
        [model["id"], model.get("generation", 0), mappings, changes, publishable],
        sort_keys=True, ensure_ascii=True,
    ).encode()).hexdigest()
    return {"changes": changes, "token": token, "applicable": bool(changes or publishable)}, mappings


def preview_mapping_reset(store, model_id):
    preview, _ = _recommended_reset(store, store.model(model_id))
    return preview


def reset_mapping(store, model_id, token):
    old = store.model(model_id)
    preview, mappings = _recommended_reset(store, old)
    if preview["token"] != token:
        raise EngineError("The label mappings changed. Preview the reset again.")
    if not preview["applicable"]:
        raise EngineError("The saved mappings already match the recommendations.")
    return _configure(store, old, mappings)


def configure(store, model_id, settings):
    return _configure(store, store.model(model_id), settings)


def _configure(store, old, settings):
    model_id = old["id"]
    candidates = candidates_for(old)
    supported = {candidate["adapter"] for candidate in candidates if candidate["adapter"] in ADAPTER_TASKS}
    if not isinstance(settings, dict) or not supported or set(settings) != supported:
        raise EngineError("Provide a label mapping for each configurable adapter.")
    updated = []
    for candidate in candidates:
        adapter = candidate["adapter"]
        if adapter in settings:
            overrides = settings[adapter]
            resolve_labels(candidate["labels"], ADAPTER_TASKS[adapter], overrides)
            candidate = {**candidate, "label_map": overrides}
        updated.append(candidate)
    approved = store.label_approvals(exclude_model=model_id)
    configured = {**old, **resolved_metadata(old, updated, old["files"], approved)}
    return store.update(old, configured, lambda: None, approvals=approved_targets(configured))
