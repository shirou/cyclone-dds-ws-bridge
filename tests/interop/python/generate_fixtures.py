#!/usr/bin/env python3
"""Generate CDR fixtures from Python (pycdr2) for cross-language interop testing.

Each fixture is a .bin file containing the XCDR2-encoded CDR bytes (including
the 4-byte encapsulation header). The Go interop tests decode these fixtures
and compare the values.
"""

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

FIXTURES = {
    "sensor_data": SensorData(
        sensor_id=42,
        temperature=23.5,
        humidity=65.2,
        location="room-101",
        active=True,
    ),
    "primitive_types_boundary": PrimitiveTypes(
        val_bool=True,
        val_octet=255,
        val_short=-32768,
        val_ushort=65535,
        val_long=-2147483648,
        val_ulong=4294967295,
        val_longlong=-9223372036854775808,
        val_ulonglong=18446744073709551615,
        val_float=3.4028235e+38,
        val_double=1.7976931348623157e+308,
        val_string="boundary",
    ),
    "primitive_types_zero": PrimitiveTypes(
        val_bool=False,
        val_octet=0,
        val_short=0,
        val_ushort=0,
        val_long=0,
        val_ulong=0,
        val_longlong=0,
        val_ulonglong=0,
        val_float=0.0,
        val_double=0.0,
        val_string="",
    ),
    "keyed_message": KeyedMessage(
        id=100,
        topic="sensor/temperature",
        payload='{"value": 22.5}',
        timestamp_ns=1700000000000000000,
    ),
    "with_sequence_nonempty": WithSequence(
        count=5,
        values=[10, 20, 30, 40, 50],
        label="test-sequence",
    ),
    "with_sequence_empty": WithSequence(
        count=0,
        values=[],
        label="empty",
    ),
    "with_enum_red": WithEnum(
        id=1,
        color=Color.RED,
        description="red item",
    ),
    "with_enum_blue": WithEnum(
        id=3,
        color=Color.BLUE,
        description="blue item",
    ),
}


def write_fixture(name: str, data) -> None:
    """Serialize an IDL struct to a .bin fixture file."""
    cdr_bytes = data.serialize(use_version_2=True)
    path = os.path.join(FIXTURE_DIR, f"py_{name}.bin")
    with open(path, "wb") as f:
        f.write(cdr_bytes)
    print(f"  wrote {path} ({len(cdr_bytes)} bytes)")


def main() -> int:
    os.makedirs(FIXTURE_DIR, exist_ok=True)

    print("Generating Python CDR fixtures:")
    for name, data in FIXTURES.items():
        write_fixture(name, data)

    print(f"\nGenerated {len(FIXTURES)} fixtures in {FIXTURE_DIR}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
