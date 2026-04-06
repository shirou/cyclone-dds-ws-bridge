"""Shared IDL type definitions for interop testing (matching idl/*.idl)."""

from dataclasses import dataclass

from pycdr2 import IdlStruct
from pycdr2.types import (
    uint8,
    int16,
    uint16,
    int32,
    uint32,
    int64,
    uint64,
    float32,
    float64,
    sequence,
)
from pycdr2 import Enum


@dataclass
class SensorData(IdlStruct, typename="sensor::SensorData"):
    sensor_id: int32
    temperature: float64
    humidity: float32
    location: str
    active: bool


class Color(Enum):
    RED = 0
    GREEN = 1
    BLUE = 2


Color.__idl_annotations__ = {}


@dataclass
class PrimitiveTypes(IdlStruct, typename="interop::PrimitiveTypes"):
    val_bool: bool
    val_octet: uint8
    val_short: int16
    val_ushort: uint16
    val_long: int32
    val_ulong: uint32
    val_longlong: int64
    val_ulonglong: uint64
    val_float: float32
    val_double: float64
    val_string: str


@dataclass
class KeyedMessage(IdlStruct, typename="interop::KeyedMessage"):
    id: int32
    topic: str
    payload: str
    timestamp_ns: int64


@dataclass
class WithSequence(IdlStruct, typename="interop::WithSequence"):
    count: int32
    values: sequence[int32]
    label: str


@dataclass
class WithEnum(IdlStruct, typename="interop::WithEnum"):
    id: int32
    color: Color
    description: str
