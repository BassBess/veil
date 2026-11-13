"""Local, conservative suggestions for structured and indirect identifiers."""
import re
from veil_engine.detection_options import literal_matches

PATTERNS = {
    "POSTAL_CODE": [
        r"(?:zip(?:[ \t]+code)?|postal(?:[ \t]+code)?|postcode|code[ \t]+postal)[ \t]*[:=-]?[ \t]*([A-Z0-9][A-Z0-9 -]{2,9})(?=$|[.,;\n])",
        r"\b([A-Z]{1,2}\d[A-Z\d]?[ \t]+\d[A-Z]{2})\b",
    ],
    "STREET_ADDRESS": [
        r"(?:address|adresse|direcci[oó]n|عنوان)[ \t]*[:=][ \t]*([^\n;]{4,120})",
        r"\b(\d{1,6}[ \t]+(?:[\w'-]+[ \t]+){0,5}(?:street|st|road|rd|avenue|ave|lane|ln|boulevard|blvd|drive|dr)\.?)(?=\s|$|[,;])",
    ],
    "ID_NUMBER": [
        r"(?:passport|national[ \t]+id|patient[ \t]+id|employee[ \t]+id|medical[ \t]+record)[ \t]*(?:no\.?|number)?[ \t]*[:=#-]?[ \t]*([A-Z0-9][A-Z0-9-]{3,29})\b",
    ],
}


LABELS = {
    "en": {"POSTAL_CODE": r"zip(?:[ \t]+code)?|postal(?:[ \t]+code)?|postcode", "STREET_ADDRESS": "address", "ID_NUMBER": r"passport|national[ \t]+id|patient[ \t]+id|employee[ \t]+id|medical[ \t]+record"},
    "fr": {"POSTAL_CODE": r"code[ \t]+postal", "STREET_ADDRESS": "adresse", "ID_NUMBER": r"passeport|numéro[ \t]+de[ \t]+patient|identifiant[ \t]+patient|carte[ \t]+d'identité"},
    "es": {"POSTAL_CODE": r"código[ \t]+postal|codigo[ \t]+postal", "STREET_ADDRESS": "dirección|direccion", "ID_NUMBER": r"pasaporte|identificación|identificacion|id[ \t]+de[ \t]+paciente|dni|nie"},
    "ar": {"POSTAL_CODE": r"الرمز[ \t]+البريدي|رمز[ \t]+بريدي", "STREET_ADDRESS": "العنوان|عنوان", "ID_NUMBER": r"رقم[ \t]+جواز[ \t]+السفر|جواز[ \t]+السفر|رقم[ \t]+الهوية|رقم[ \t]+المريض"},
}
MONTHS = {
    "fr": "janvier|février|mars|avril|mai|juin|juillet|août|septembre|octobre|novembre|décembre",
    "es": "enero|febrero|marzo|abril|mayo|junio|julio|agosto|septiembre|octubre|noviembre|diciembre",
    "ar": "يناير|فبراير|مارس|أبريل|ابريل|مايو|يونيو|يوليو|أغسطس|سبتمبر|أكتوبر|نوفمبر|ديسمبر|كانون الثاني|شباط|آذار|نيسان|أيار|حزيران|تموز|آب|أيلول|تشرين الأول|تشرين الثاني|كانون الأول",
}


def suggestions(text, enabled, rules, doc=None, language="en"):

    result = []

    def add(category, start, end, confidence, source="context rule"):
        if category in enabled and start < end:
            result.append({"entity_type": category, "text": text[start:end],
                           "start": start, "end": end, "confidence": confidence, "source": source})

    patterns_by_category = {category: list(patterns) if language == "en" else [] for category, patterns in PATTERNS.items()}
    for category, label in LABELS[language].items():
        value = {"POSTAL_CODE": r"([\w][\w -]{1,11})", "STREET_ADDRESS": r"([^\n;؛]{4,120})", "ID_NUMBER": r"([\w][\w-]{3,29})"}[category]
        patterns_by_category[category].append(r"(?:" + label + r")[ \t]*[:=#-][ \t]*" + value + r"(?=$|[\s.,;؛])")
    if language in MONTHS:
        patterns_by_category["DATE_TIME"] = [r"(?<!\w)(\d{1,2}\s+(?:de\s+)?(?:" + MONTHS[language] + r")(?:\s+(?:de\s+)?\d{4})?)(?!\w)"]
    for category, patterns in patterns_by_category.items():
        if category in enabled:
            for pattern in patterns:
                for match in re.finditer(pattern, text, re.IGNORECASE):
                    start, end = match.span(1)
                    while end > start and text[end - 1].isspace():
                        end -= 1
                    if category in {"ID_NUMBER", "POSTAL_CODE"} and not any(char.isdigit() for char in text[start:end]):
                        continue
                    add(category, start, end, .7)

    if doc is not None:
        if "ORGANIZATION" in enabled:
            for entity in doc.ents:
                if entity.label_ == "ORG":
                    add("ORGANIZATION", entity.start_char, entity.end_char, .75)
        if "INDIRECT_IDENTIFIER" in enabled and language == "en" and doc.has_annotation("DEP"):
            nationalities = [entity for entity in doc.ents if entity.label_ == "NORP"]
            for chunk in doc.noun_chunks:
                unique = any(token.lower_ in {"only", "sole", "unique"} for token in chunk)
                has_nationality = any(chunk.start_char <= entity.start_char
                                      and entity.end_char <= chunk.end_char for entity in nationalities)
                if chunk.root.pos_ == "NOUN" and (unique or has_nationality):
                    end = chunk.end_char
                    for child in chunk.root.children:
                        if child.dep_ == "prep":
                            end = max(end, child.right_edge.idx + len(child.right_edge))
                    add("INDIRECT_IDENTIFIER", chunk.start_char, end, .6)

    for rule in rules or []:
        category = rule.get("entity_type")
        phrase = rule.get("phrase", "")
        if category not in enabled or not isinstance(phrase, str) or not phrase.strip():
            continue
        # Literal phrases, never user-supplied regex or executable rules.
        for match in literal_matches(text, phrase):
            add(category, match.start(), match.end(), .95, "custom phrase")
    return result
