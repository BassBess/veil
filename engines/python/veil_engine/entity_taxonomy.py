"""Shared offline names for Veil entity categories."""
import hashlib
import re
import unicodedata

from veil_engine.catalog import SUPPORTED_ENTITIES

CUSTOM = re.compile(r"CUSTOM_[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)*")

# Alias only equivalent labels. Keep distinct credentials, identifiers, and address parts separate.
TAXONOMY = {
    "PERSON": ("PER", "PERSON_NAME", "FULL_NAME", "NAME_OF_PERSON", "HUMAN_NAME"),
    "ORGANIZATION": ("ORG", "ORGANISATION", "COMPANY", "COMPANY_NAME", "ORGANIZATION_NAME"),
    "LOCATION": ("LOC", "GPE", "GEOGRAPHIC_LOCATION", "PLACE"),
    "EMAIL_ADDRESS": ("EMAIL", "E_MAIL", "E_MAIL_ADDRESS", "EMAIL_ID", "MAIL_ADDRESS"),
    "PHONE_NUMBER": ("PHONE", "TELEPHONE", "TELEPHONE_NUMBER", "TEL", "TEL_NUMBER", "CONTACT_PHONE",
                     "MOBILE", "MOBILE_NUMBER", "CELL_PHONE"),
    "STREET_ADDRESS": ("ADDRESS", "MAILING_ADDRESS", "POSTAL_ADDRESS", "ADDRESS_LINE_1"),
    "POSTAL_CODE": ("POSTCODE", "POST_CODE", "ZIP", "ZIP_CODE", "ZIPCODE"),
    "DATE_TIME": ("DATE", "DATETIME", "CALENDAR_DATE"),
    "CREDIT_CARD": ("CREDIT_DEBIT_CARD", "PAYMENT_CARD", "PAYMENT_CARD_NUMBER", "CREDIT_CARD_NUMBER",
                    "DEBIT_CARD", "DEBIT_CARD_NUMBER", "CARD_NUMBER", "PRIMARY_ACCOUNT_NUMBER"),
    "IBAN_CODE": ("IBAN", "IBAN_NUMBER", "INTERNATIONAL_BANK_ACCOUNT_NUMBER"),
    "IP_ADDRESS": ("IP", "IPV4", "IPV6", "INTERNET_PROTOCOL_ADDRESS"),
    "URL": ("URI", "WEB_URL", "WEB_ADDRESS", "WEBSITE_URL"),
    "ID_NUMBER": ("IDENTIFICATION_NUMBER", "GENERIC_ID", "IDENTIFIER_NUMBER"),
    "INDIRECT_IDENTIFIER": ("QUASI_IDENTIFIER",),
    "CUSTOM_GIVEN_NAME": ("GIVEN_NAME", "FIRST_NAME", "FORENAME"),
    "CUSTOM_FAMILY_NAME": ("FAMILY_NAME", "SURNAME", "LAST_NAME"),
    "CUSTOM_USERNAME": ("USERNAME", "USER_NAME", "LOGIN_NAME", "USER_HANDLE"),
    "CUSTOM_AGE": ("AGE", "AGE_YEARS"),
    "CUSTOM_GENDER": ("GENDER",),
    "CUSTOM_DATE_OF_BIRTH": ("DATE_OF_BIRTH", "DOB", "BIRTH_DATE", "BIRTHDAY"),
    "CUSTOM_TIME": ("TIME", "TIME_OF_DAY", "CLOCK_TIME"),
    "CUSTOM_CITY": ("CITY", "CITY_NAME", "MUNICIPALITY"),
    "CUSTOM_STATE_PROVINCE": ("STATE", "PROVINCE", "STATE_PROVINCE", "REGION_STATE"),
    "CUSTOM_COUNTRY": ("COUNTRY", "COUNTRY_NAME"),
    "CUSTOM_COORDINATE": ("COORDINATE", "COORDINATES", "GEO_COORDINATES", "LAT_LONG", "GPS_COORDINATES"),
    "CUSTOM_STREET_NAME": ("STREET_NAME", "ROAD_NAME"),
    "CUSTOM_BUILDING_NUMBER": ("BUILDING_NUMBER", "HOUSE_NUMBER", "STREET_NUMBER"),
    "CUSTOM_SECONDARY_ADDRESS": ("SECONDARY_ADDRESS", "ADDRESS_LINE_2", "UNIT_NUMBER", "APARTMENT_NUMBER"),
    "CUSTOM_FAX_NUMBER": ("FAX", "FAX_NUMBER"),
    "CUSTOM_ACCOUNT_ID": ("ACCOUNT_ID", "ACCOUNT_IDENTIFIER"),
    "CUSTOM_ACCOUNT_NUMBER": ("ACCOUNT_NUMBER", "ACCOUNT_NO"),
    "CUSTOM_BANK_ACCOUNT": ("BANK_ACCOUNT", "BANK_ACCOUNT_NUMBER", "BANK_ACCOUNT_NO"),
    "CUSTOM_ROUTING_NUMBER": ("ROUTING_NUMBER", "BANK_ROUTING_NUMBER", "ABA_ROUTING_NUMBER", "ABA_NUMBER"),
    "CUSTOM_SWIFT_BIC": ("SWIFT_BIC", "SWIFT_CODE", "BIC", "BANK_IDENTIFIER_CODE"),
    "CUSTOM_CREDIT_CARD_CVV": ("CREDIT_CARD_CVV", "CVV", "CVV2", "CVC", "CVC2", "CARD_SECURITY_CODE"),
    "CUSTOM_CUSTOMER_ID": ("CUSTOMER_ID", "CLIENT_ID", "CUSTOMER_NUMBER"),
    "CUSTOM_EMPLOYEE_ID": ("EMPLOYEE_ID", "STAFF_ID", "EMPLOYEE_NUMBER"),
    "CUSTOM_GOVERNMENT_ID": ("GOVERNMENT_ID", "GOVT_ID", "GOVERNMENT_IDENTIFIER"),
    "CUSTOM_NATIONAL_ID": ("NATIONAL_ID", "NATIONAL_ID_NUMBER", "NATIONAL_IDENTIFICATION_NUMBER"),
    "CUSTOM_SSN": ("SSN", "SOCIAL_SECURITY_NUMBER", "SOCIAL_SECURITY_NO"),
    "CUSTOM_TAX_ID": ("TAX_ID", "TAX_IDENTIFIER", "TAX_IDENTIFICATION_NUMBER", "TIN"),
    "CUSTOM_PASSPORT": ("PASSPORT", "PASSPORT_NO", "PASSPORT_NUMBER"),
    "CUSTOM_DRIVER_LICENSE": ("DRIVER_LICENSE", "DRIVERS_LICENSE", "DRIVING_LICENCE",
                              "DRIVER_LICENCE_NUMBER", "DRIVER_LICENSE_NUMBER"),
    "CUSTOM_LICENSE_NUMBER": ("LICENSE_NUMBER", "LICENCE_NUMBER"),
    "CUSTOM_MEDICAL_ID": ("MEDICAL_ID", "MEDICAL_IDENTIFIER"),
    "CUSTOM_MEDICAL_RECORD_NUMBER": ("MEDICAL_RECORD_NUMBER", "MEDICAL_RECORD_ID", "MRN"),
    "CUSTOM_VEHICLE_ID": ("VEHICLE_ID", "VEHICLE_IDENTIFIER"),
    "CUSTOM_LICENSE_PLATE": ("LICENSE_PLATE", "LICENCE_PLATE", "NUMBER_PLATE", "VEHICLE_REGISTRATION_PLATE"),
    "CUSTOM_MAC_ADDRESS": ("MAC_ADDRESS", "MAC", "HARDWARE_ADDRESS"),
    "CUSTOM_DEVICE_ID": ("DEVICE_ID", "DEVICE_IDENTIFIER"),
    "CUSTOM_PASSWORD": ("PASSWORD", "PASSWD", "PASSPHRASE"),
    "CUSTOM_PIN": ("PIN", "PIN_CODE", "PERSONAL_IDENTIFICATION_NUMBER"),
    "CUSTOM_API_KEY": ("API_KEY", "APIKEY", "API_ACCESS_KEY"),
    "CUSTOM_SECRET": ("SECRET", "GENERIC_SECRET"),
}
CATEGORIES = SUPPORTED_ENTITIES | TAXONOMY.keys()
OPAQUE = re.compile(r"(?:(?:LABEL|CLASS|ENTITY|TAG|CATEGORY|PII)_?)?\d+|UNKNOWN|OTHER|UNDEFINED|MISC|UNSPECIFIED")


def label_key(value):
    value = unicodedata.normalize("NFKC", value).strip()
    value = re.sub(r"([A-Z])([A-Z][a-z])", r"\1_\2", value)
    value = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", value)
    return re.sub(r"[\W_]+", "_", value.upper()).strip("_")


def _aliases():
    exact, compact = {}, {}
    for target, aliases in TAXONOMY.items():
        for alias in (target, *aliases):
            key = label_key(alias)
            if key in exact and exact[key] != target:
                raise RuntimeError(f"Conflicting taxonomy entry for {alias}")
            exact[key] = target
            squashed = key.replace("_", "")
            compact[squashed] = target if squashed not in compact or compact[squashed] == target else None
    return exact, compact


_EXACT, _COMPACT = _aliases()


def known_category(key):
    return _EXACT.get(key) or _COMPACT.get(key.replace("_", ""))


def suggested_custom(key, *, scope=""):
    stem = re.sub(r"[^A-Z0-9_]+", "_", key).strip("_")
    stem = re.sub(r"_+", "_", stem)
    if not stem or not stem[0].isalpha():
        stem = f"LABEL_{stem}".rstrip("_")
    code = f"CUSTOM_{stem}"
    if not scope and stem == key and len(code) <= 39 and code not in CATEGORIES:
        return code
    # Keep long, non-ASCII, or opaque labels distinct without order-dependent suffixes.
    digest = hashlib.sha256(f"{scope}\0{key}".encode()).hexdigest()[:14].upper()
    return f"CUSTOM_{stem[:17].rstrip('_')}_{digest}"
