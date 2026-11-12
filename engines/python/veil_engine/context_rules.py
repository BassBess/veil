"""Local, conservative suggestions for structured and indirect identifiers."""
import re

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


def suggestions(text, enabled, rules, doc=None):
    result = []

    def add(category, start, end, confidence):
        if category in enabled and start < end:
            result.append({"entity_type": category, "text": text[start:end],
                           "start": start, "end": end, "confidence": confidence})

    for category, patterns in PATTERNS.items():
        if category in enabled:
            for pattern in patterns:
                for match in re.finditer(pattern, text, re.IGNORECASE):
                    start, end = match.span(1)
                    while end > start and text[end - 1].isspace():
                        end -= 1
                    add(category, start, end, .7)

    if doc is not None:
        if "ORGANIZATION" in enabled:
            for entity in doc.ents:
                if entity.label_ == "ORG":
                    add("ORGANIZATION", entity.start_char, entity.end_char, .75)
        if "INDIRECT_IDENTIFIER" in enabled:
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
        for match in re.finditer(re.escape(phrase.strip()), text, re.IGNORECASE):
            if (match.start() and text[match.start() - 1].isalnum()
                    or match.end() < len(text) and text[match.end()].isalnum()):
                continue
            add(category, match.start(), match.end(), .95)
    return result
