#!/usr/bin/env python3
"""Verify Go-generated CDR fixtures can be deserialized by Python (pycdr2).

Reads go_*.bin files from the fixtures directory, deserializes them using
pycdr2, and compares the values against expected test data.
"""

import math
import os
import sys

from idl_types import (
    Color,
    KeyedMessage,
    PrimitiveTypes,
    SensorData,
    WithEnum,
    WithSequence,
)

FIXTURE_DIR = os.environ.get(
    "FIXTURE_DIR",
    os.path.join(os.path.dirname(__file__), "..", "fixtures"),
)


def float_eq(a: float, b: float, rel_tol: float = 1e-6) -> bool:
    """Compare floats with relative tolerance."""
    if math.isinf(a) and math.isinf(b):
        return (a > 0) == (b > 0)
    if math.isnan(a) and math.isnan(b):
        return True
    return math.isclose(a, b, rel_tol=rel_tol)


VERIFIERS: dict[str, tuple[type, dict]] = {
    "sensor_data": (
        SensorData,
        {
            "sensor_id": 42,
            "temperature": 23.5,
            "humidity": lambda v: float_eq(v, 65.2, rel_tol=1e-5),
            "location": "room-101",
            "active": True,
        },
    ),
    "primitive_types_boundary": (
        PrimitiveTypes,
        {
            "val_bool": True,
            "val_octet": 255,
            "val_short": -32768,
            "val_ushort": 65535,
            "val_long": -2147483648,
            "val_ulong": 4294967295,
            "val_longlong": -9223372036854775808,
            "val_ulonglong": 18446744073709551615,
            "val_float": lambda v: float_eq(v, 3.4028235e38),
            "val_double": lambda v: float_eq(v, 1.7976931348623157e308),
            "val_string": "boundary",
        },
    ),
    "primitive_types_zero": (
        PrimitiveTypes,
        {
            "val_bool": False,
            "val_octet": 0,
            "val_short": 0,
            "val_ushort": 0,
            "val_long": 0,
            "val_ulong": 0,
            "val_longlong": 0,
            "val_ulonglong": 0,
            "val_float": 0.0,
            "val_double": 0.0,
            "val_string": "",
        },
    ),
    "keyed_message": (
        KeyedMessage,
        {
            "id": 100,
            "topic": "sensor/temperature",
            "payload": '{"value": 22.5}',
            "timestamp_ns": 1700000000000000000,
        },
    ),
    "with_sequence_nonempty": (
        WithSequence,
        {
            "count": 5,
            "values": [10, 20, 30, 40, 50],
            "label": "test-sequence",
        },
    ),
    "with_sequence_empty": (
        WithSequence,
        {
            "count": 0,
            "values": [],
            "label": "empty",
        },
    ),
    "with_enum_red": (
        WithEnum,
        {
            "id": 1,
            "color": Color.RED,
            "description": "red item",
        },
    ),
    "with_enum_blue": (
        WithEnum,
        {
            "id": 3,
            "color": Color.BLUE,
            "description": "blue item",
        },
    ),
}


def verify_fixture(name: str, cls: type, expected: dict) -> bool:
    """Verify a single Go-generated fixture."""
    path = os.path.join(FIXTURE_DIR, f"go_{name}.bin")
    if not os.path.exists(path):
        print(f"  SKIP {name}: {path} not found")
        return True

    with open(path, "rb") as f:
        data = f.read()

    try:
        obj = cls.deserialize(data)
    except Exception as e:
        print(f"  FAIL {name}: deserialization error: {e}")
        return False

    ok = True
    for field, exp in expected.items():
        actual = getattr(obj, field)
        if callable(exp) and not isinstance(
            exp, (int, float, str, bool, list, type, Color)
        ):
            if not exp(actual):
                print(f"  FAIL {name}.{field}: predicate failed, got {actual!r}")
                ok = False
        elif isinstance(exp, list):
            if list(actual) != exp:
                print(f"  FAIL {name}.{field}: expected {exp!r}, got {list(actual)!r}")
                ok = False
        elif isinstance(exp, float):
            if not float_eq(actual, exp):
                print(f"  FAIL {name}.{field}: expected {exp!r}, got {actual!r}")
                ok = False
        elif actual != exp:
            print(f"  FAIL {name}.{field}: expected {exp!r}, got {actual!r}")
            ok = False

    if ok:
        print(f"  OK   {name}")
    return ok


def main() -> int:
    print("Verifying Go CDR fixtures with Python (pycdr2):")
    all_ok = True
    for name, (cls, expected) in VERIFIERS.items():
        if not verify_fixture(name, cls, expected):
            all_ok = False

    if all_ok:
        print(f"\nAll {len(VERIFIERS)} verifications passed.")
        return 0
    else:
        print("\nSome verifications FAILED.")
        return 1


if __name__ == "__main__":
    sys.exit(main())
