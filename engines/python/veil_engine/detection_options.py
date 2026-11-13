"""Validated, document-scoped detection controls. All offsets reference the source."""
import re
import unicodedata


def literal_matches(text, phrase):
    def word(char):
        return char.isalnum() or char == "_" or unicodedata.category(char).startswith("M")

    for match in re.finditer(re.escape(phrase.strip()), text, re.IGNORECASE):
        if (match.start() and word(text[match.start() - 1])
                or match.end() < len(text) and word(text[match.end()])):
            continue
        yield match


def validate_options(options, categories):
    options = options or {}
    for name in ("session_rules", "ignored_phrases", "required_context"):
        rules = options.get(name, [])
        if not isinstance(rules, list) or len(rules) > 200:
            raise ValueError("Use at most 200 rules of each type.")
        for rule in rules:
            if (not isinstance(rule, dict) or rule.get("entity_type") not in categories
                    or not isinstance(rule.get("phrase"), str) or not rule["phrase"].strip()
                    or len(rule["phrase"].encode("utf-8")) > 500):
                raise ValueError("Rules need a valid category and a phrase of 1–500 UTF-8 bytes.")
    return options


def filter_candidates(text, candidates, options):
    labels = {
        "en": {"email", "mail", "phone", "telephone", "address", "passport", "date"},
        "fr": {"courriel", "email", "téléphone", "adresse", "passeport", "date"},
        "es": {"correo", "email", "teléfono", "dirección", "pasaporte", "fecha"},
        "ar": {"بريد", "البريد الإلكتروني", "الهاتف", "هاتف", "العنوان", "عنوان", "تاريخ"},
    }[options.get("language", "en")]
    exclusions = {}
    for rule in options.get("ignored_phrases", []):
        exclusions.setdefault(rule["entity_type"], set()).add(rule["phrase"].strip().casefold())
    contexts = {}
    for rule in options.get("required_context", []):
        contexts.setdefault(rule["entity_type"], []).extend(literal_matches(text, rule["phrase"]))
    required_categories = {rule["entity_type"] for rule in options.get("required_context", [])}
    accepted, suppressed = [], []
    for item in candidates:
        reason = None
        if (item.get("source") != "custom phrase"
                and item["entity_type"] in {"PERSON", "LOCATION", "ORGANIZATION"}
                and item["text"].strip().casefold() in labels
                and re.match(r"[ \t]*[:：]", text[item["end"]:])):
            reason = "Field label, not an identity"
        elif item["text"].strip().casefold() in exclusions.get(item["entity_type"], set()):
            reason = "Ignored phrase"
        elif item["entity_type"] in required_categories and not any(
            hit.start() <= item["end"] + 80 and hit.end() >= item["start"] - 80
            # Context may not cross lines or sentence punctuation.
            and not re.search(r"[\n.!?؟؛]", text[min(hit.end(), item["end"]):max(hit.start(), item["start"])])
            for hit in contexts.get(item["entity_type"], [])
        ):
            reason = "Required context not found nearby"
        if reason:
            suppressed.append({**item, "reason": reason})
        else:
            accepted.append(item)
    return accepted, suppressed
