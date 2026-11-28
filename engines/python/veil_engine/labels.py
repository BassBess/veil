"""Label resolution for local adapters. Decisions are keyed by native entity group."""
import re

from veil_engine.catalog import SUPPORTED_ENTITIES
from veil_engine.components import Capability, EngineError
from veil_engine.entity_taxonomy import CUSTOM, OPAQUE, known_category, label_key, suggested_custom

IGNORE = "@ignore"
BOUNDARIES = {"B": "B", "I": "I", "L": "L", "U": "U", "E": "L", "S": "U"}


def is_custom(value):
    return isinstance(value, str) and len(value) <= 39 and CUSTOM.fullmatch(value) is not None


def group_label(label, token):
    # Persisted model overrides use these keys. Normalize more spelling variants only for lookup.
    name = re.sub(r"[-\s]+", "_", label.strip().upper())
    marker, _, group = name.partition("_")
    if token and marker in BOUNDARIES and group:
        return BOUNDARIES[marker], group
    return ("I" if token else ""), name


def label_identity(label, token, group):
    trimmed = label.strip()
    if token:
        trimmed = re.sub(r"^[BILUES][_.:/\s-]+(?=.)", "", trimmed, count=1, flags=re.IGNORECASE)
    key = label_key(trimmed)
    category = known_category(key) or (group if is_custom(group) else None)
    opaque = not key or OPAQUE.fullmatch(key) is not None
    if opaque:
        reuse_key = None
    else:
        reuse_key = f"category:{category}" if category else f"label:{key}"
    return key or group, category, reuse_key


def resolve_labels(labels, task, overrides, *, approved=None, defaults=None, scope=""):
    token = task == Capability.DETECT
    groups = {}
    positions = []
    approved, defaults = approved or {}, defaults or {}
    if not isinstance(overrides, dict):
        raise EngineError("Use an object of label mapping decisions.")
    for label in labels:
        boundary, group = group_label(label, token)
        if not group:
            raise EngineError("A model label has an empty entity group.")
        if group == "O":
            positions.append("O")
            continue
        key, category, reuse_key = label_identity(label, token, group)
        learned = approved.get(reuse_key, category)
        if reuse_key in approved:
            reason = "conflict" if learned is None else "approved"
        else:
            reason = "taxonomy" if category else "custom"
        suggested = learned or category or suggested_custom(key, scope=scope if reuse_key is None else "")
        default = defaults.get(group, learned)
        target = overrides.get(group, default)
        if target is not None and (not isinstance(target, str) or
                target != IGNORE and target not in SUPPORTED_ENTITIES and not is_custom(target)):
            raise EngineError("Map labels to a Veil category, a CUSTOM_ code, or explicit Ignore.")
        groups[group] = {"label": group, "automatic": default, "target": target,
                         "suggested": suggested, "reason": reason, "reuse_key": reuse_key}
        if target is None:
            positions.append(None)
        elif target == IGNORE:
            positions.append("O")
        else:
            positions.append(f"{boundary}-{target}" if token else target)
    if set(overrides) - set(groups) or any(value is None for value in overrides.values()):
        raise EngineError("The label mapping contains an unknown group or empty decision.")
    return positions, list(groups.values())
