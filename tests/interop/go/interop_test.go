package main

import (
	"bytes"
	"math"
	"os"
	"path/filepath"
	"testing"

	"github.com/shirou/cyclone-dds-ws-bridge/tests/interop/go/interop"
	"github.com/shirou/cyclone-dds-ws-bridge/tests/interop/go/sensor"
)

const fixtureDir = "../fixtures"

// testSensorData is the shared test value for sensor::SensorData.
var testSensorData = sensor.SensorData{
	SensorID:    42,
	Temperature: 23.5,
	Humidity:    65.2,
	Location:    "room-101",
	Active:      true,
}

var testPrimitiveTypesBoundary = interop.PrimitiveTypes{
	ValBool:      true,
	ValOctet:     255,
	ValShort:     -32768,
	ValUshort:    65535,
	ValLong:      -2147483648,
	ValUlong:     4294967295,
	ValLonglong:  -9223372036854775808,
	ValUlonglong: 18446744073709551615,
	ValFloat:     math.MaxFloat32,
	ValDouble:    math.MaxFloat64,
	ValString:    "boundary",
}

var testPrimitiveTypesZero = interop.PrimitiveTypes{
	ValString: "",
}

var testKeyedMessage = interop.KeyedMessage{
	ID:          100,
	Topic:       "sensor/temperature",
	Payload:     `{"value": 22.5}`,
	TimestampNs: 1700000000000000000,
}

var testWithSequenceNonempty = interop.WithSequence{
	Count:  5,
	Values: []int32{10, 20, 30, 40, 50},
	Label:  "test-sequence",
}

var testWithSequenceEmpty = interop.WithSequence{
	Count:  0,
	Values: []int32{},
	Label:  "empty",
}

var testWithEnumRed = interop.WithEnum{
	ID:          1,
	Color:       interop.ColorRED,
	Description: "red item",
}

var testWithEnumBlue = interop.WithEnum{
	ID:          3,
	Color:       interop.ColorBLUE,
	Description: "blue item",
}

type marshalCase struct {
	name string
	data interface {
		MarshalCDR() ([]byte, error)
		UnmarshalCDR([]byte) error
	}
}

func allCases() []marshalCase {
	return []marshalCase{
		{"sensor_data", &testSensorData},
		{"primitive_types_boundary", &testPrimitiveTypesBoundary},
		{"primitive_types_zero", &testPrimitiveTypesZero},
		{"keyed_message", &testKeyedMessage},
		{"with_sequence_nonempty", &testWithSequenceNonempty},
		{"with_sequence_empty", &testWithSequenceEmpty},
		{"with_enum_red", &testWithEnumRed},
		{"with_enum_blue", &testWithEnumBlue},
	}
}

// TestGoMarshalRoundTrip verifies Go marshal -> unmarshal produces identical values.
func TestGoMarshalRoundTrip(t *testing.T) {
	cases := []struct {
		name     string
		marshal  func() ([]byte, error)
		verify   func([]byte) error
	}{
		{"sensor_data", testSensorData.MarshalCDR, func(b []byte) error {
			var v sensor.SensorData
			if err := v.UnmarshalCDR(b); err != nil {
				return err
			}
			if v != testSensorData {
				t.Errorf("round-trip mismatch: got %+v", v)
			}
			return nil
		}},
		{"primitive_types_boundary", testPrimitiveTypesBoundary.MarshalCDR, func(b []byte) error {
			var v interop.PrimitiveTypes
			if err := v.UnmarshalCDR(b); err != nil {
				return err
			}
			assertPrimitiveTypes(t, &v, &testPrimitiveTypesBoundary)
			return nil
		}},
		{"primitive_types_zero", testPrimitiveTypesZero.MarshalCDR, func(b []byte) error {
			var v interop.PrimitiveTypes
			if err := v.UnmarshalCDR(b); err != nil {
				return err
			}
			assertPrimitiveTypes(t, &v, &testPrimitiveTypesZero)
			return nil
		}},
		{"keyed_message", testKeyedMessage.MarshalCDR, func(b []byte) error {
			var v interop.KeyedMessage
			if err := v.UnmarshalCDR(b); err != nil {
				return err
			}
			if v != testKeyedMessage {
				t.Errorf("round-trip mismatch: got %+v", v)
			}
			return nil
		}},
		{"with_sequence_nonempty", testWithSequenceNonempty.MarshalCDR, func(b []byte) error {
			var v interop.WithSequence
			if err := v.UnmarshalCDR(b); err != nil {
				return err
			}
			assertWithSequence(t, &v, &testWithSequenceNonempty)
			return nil
		}},
		{"with_sequence_empty", testWithSequenceEmpty.MarshalCDR, func(b []byte) error {
			var v interop.WithSequence
			if err := v.UnmarshalCDR(b); err != nil {
				return err
			}
			assertWithSequence(t, &v, &testWithSequenceEmpty)
			return nil
		}},
		{"with_enum_red", testWithEnumRed.MarshalCDR, func(b []byte) error {
			var v interop.WithEnum
			if err := v.UnmarshalCDR(b); err != nil {
				return err
			}
			if v != testWithEnumRed {
				t.Errorf("round-trip mismatch: got %+v", v)
			}
			return nil
		}},
		{"with_enum_blue", testWithEnumBlue.MarshalCDR, func(b []byte) error {
			var v interop.WithEnum
			if err := v.UnmarshalCDR(b); err != nil {
				return err
			}
			if v != testWithEnumBlue {
				t.Errorf("round-trip mismatch: got %+v", v)
			}
			return nil
		}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			data, err := tc.marshal()
			if err != nil {
				t.Fatalf("MarshalCDR: %v", err)
			}
			if err := tc.verify(data); err != nil {
				t.Fatalf("UnmarshalCDR: %v", err)
			}
		})
	}
}

// TestDecodePythonFixtures decodes Python-generated .bin fixtures and verifies values.
func TestDecodePythonFixtures(t *testing.T) {
	for _, tc := range allCases() {
		t.Run(tc.name, func(t *testing.T) {
			path := filepath.Join(fixtureDir, "py_"+tc.name+".bin")
			data, err := os.ReadFile(path)
			if err != nil {
				t.Skipf("fixture not found: %s", path)
			}

			switch tc.name {
			case "sensor_data":
				var v sensor.SensorData
				if err := v.UnmarshalCDR(data); err != nil {
					t.Fatalf("UnmarshalCDR: %v", err)
				}
				assertSensorData(t, &v, &testSensorData)

			case "primitive_types_boundary":
				var v interop.PrimitiveTypes
				if err := v.UnmarshalCDR(data); err != nil {
					t.Fatalf("UnmarshalCDR: %v", err)
				}
				assertPrimitiveTypes(t, &v, &testPrimitiveTypesBoundary)

			case "primitive_types_zero":
				var v interop.PrimitiveTypes
				if err := v.UnmarshalCDR(data); err != nil {
					t.Fatalf("UnmarshalCDR: %v", err)
				}
				assertPrimitiveTypes(t, &v, &testPrimitiveTypesZero)

			case "keyed_message":
				var v interop.KeyedMessage
				if err := v.UnmarshalCDR(data); err != nil {
					t.Fatalf("UnmarshalCDR: %v", err)
				}
				assertKeyedMessage(t, &v, &testKeyedMessage)

			case "with_sequence_nonempty":
				var v interop.WithSequence
				if err := v.UnmarshalCDR(data); err != nil {
					t.Fatalf("UnmarshalCDR: %v", err)
				}
				assertWithSequence(t, &v, &testWithSequenceNonempty)

			case "with_sequence_empty":
				var v interop.WithSequence
				if err := v.UnmarshalCDR(data); err != nil {
					t.Fatalf("UnmarshalCDR: %v", err)
				}
				assertWithSequence(t, &v, &testWithSequenceEmpty)

			case "with_enum_red":
				var v interop.WithEnum
				if err := v.UnmarshalCDR(data); err != nil {
					t.Fatalf("UnmarshalCDR: %v", err)
				}
				assertWithEnum(t, &v, &testWithEnumRed)

			case "with_enum_blue":
				var v interop.WithEnum
				if err := v.UnmarshalCDR(data); err != nil {
					t.Fatalf("UnmarshalCDR: %v", err)
				}
				assertWithEnum(t, &v, &testWithEnumBlue)
			}
		})
	}
}

// TestGenerateGoFixtures generates Go .bin fixtures for Python to verify.
func TestGenerateGoFixtures(t *testing.T) {
	if err := os.MkdirAll(fixtureDir, 0o755); err != nil {
		t.Fatalf("mkdir: %v", err)
	}

	for _, tc := range allCases() {
		t.Run(tc.name, func(t *testing.T) {
			data, err := tc.data.MarshalCDR()
			if err != nil {
				t.Fatalf("MarshalCDR: %v", err)
			}
			path := filepath.Join(fixtureDir, "go_"+tc.name+".bin")
			if err := os.WriteFile(path, data, 0o644); err != nil {
				t.Fatalf("write: %v", err)
			}
			t.Logf("wrote %s (%d bytes)", path, len(data))
		})
	}
}

// TestByteExactComparison verifies Go and Python produce identical CDR bytes.
func TestByteExactComparison(t *testing.T) {
	for _, tc := range allCases() {
		t.Run(tc.name, func(t *testing.T) {
			pyPath := filepath.Join(fixtureDir, "py_"+tc.name+".bin")
			pyData, err := os.ReadFile(pyPath)
			if err != nil {
				t.Skipf("Python fixture not found: %s", pyPath)
			}

			goData, err := tc.data.MarshalCDR()
			if err != nil {
				t.Fatalf("MarshalCDR: %v", err)
			}

			if !bytes.Equal(goData, pyData) {
				t.Errorf("byte mismatch for %s:\n  Go  (%d bytes): %x\n  Py  (%d bytes): %x",
					tc.name, len(goData), goData, len(pyData), pyData)
			}
		})
	}
}

// ---- assertion helpers ----

func assertSensorData(t *testing.T, got, want *sensor.SensorData) {
	t.Helper()
	if got.SensorID != want.SensorID {
		t.Errorf("SensorID: got %d, want %d", got.SensorID, want.SensorID)
	}
	if got.Temperature != want.Temperature {
		t.Errorf("Temperature: got %f, want %f", got.Temperature, want.Temperature)
	}
	if !floatEq32(got.Humidity, want.Humidity) {
		t.Errorf("Humidity: got %f, want %f", got.Humidity, want.Humidity)
	}
	if got.Location != want.Location {
		t.Errorf("Location: got %q, want %q", got.Location, want.Location)
	}
	if got.Active != want.Active {
		t.Errorf("Active: got %v, want %v", got.Active, want.Active)
	}
}

func assertPrimitiveTypes(t *testing.T, got, want *interop.PrimitiveTypes) {
	t.Helper()
	if got.ValBool != want.ValBool {
		t.Errorf("ValBool: got %v, want %v", got.ValBool, want.ValBool)
	}
	if got.ValOctet != want.ValOctet {
		t.Errorf("ValOctet: got %d, want %d", got.ValOctet, want.ValOctet)
	}
	if got.ValShort != want.ValShort {
		t.Errorf("ValShort: got %d, want %d", got.ValShort, want.ValShort)
	}
	if got.ValUshort != want.ValUshort {
		t.Errorf("ValUshort: got %d, want %d", got.ValUshort, want.ValUshort)
	}
	if got.ValLong != want.ValLong {
		t.Errorf("ValLong: got %d, want %d", got.ValLong, want.ValLong)
	}
	if got.ValUlong != want.ValUlong {
		t.Errorf("ValUlong: got %d, want %d", got.ValUlong, want.ValUlong)
	}
	if got.ValLonglong != want.ValLonglong {
		t.Errorf("ValLonglong: got %d, want %d", got.ValLonglong, want.ValLonglong)
	}
	if got.ValUlonglong != want.ValUlonglong {
		t.Errorf("ValUlonglong: got %d, want %d", got.ValUlonglong, want.ValUlonglong)
	}
	if !floatEq32(got.ValFloat, want.ValFloat) {
		t.Errorf("ValFloat: got %g, want %g", got.ValFloat, want.ValFloat)
	}
	if got.ValDouble != want.ValDouble {
		t.Errorf("ValDouble: got %g, want %g", got.ValDouble, want.ValDouble)
	}
	if got.ValString != want.ValString {
		t.Errorf("ValString: got %q, want %q", got.ValString, want.ValString)
	}
}

func assertKeyedMessage(t *testing.T, got, want *interop.KeyedMessage) {
	t.Helper()
	if got.ID != want.ID {
		t.Errorf("ID: got %d, want %d", got.ID, want.ID)
	}
	if got.Topic != want.Topic {
		t.Errorf("Topic: got %q, want %q", got.Topic, want.Topic)
	}
	if got.Payload != want.Payload {
		t.Errorf("Payload: got %q, want %q", got.Payload, want.Payload)
	}
	if got.TimestampNs != want.TimestampNs {
		t.Errorf("TimestampNs: got %d, want %d", got.TimestampNs, want.TimestampNs)
	}
}

func assertWithSequence(t *testing.T, got, want *interop.WithSequence) {
	t.Helper()
	if got.Count != want.Count {
		t.Errorf("Count: got %d, want %d", got.Count, want.Count)
	}
	if len(got.Values) != len(want.Values) {
		t.Errorf("Values length: got %d, want %d", len(got.Values), len(want.Values))
	} else {
		for i := range got.Values {
			if got.Values[i] != want.Values[i] {
				t.Errorf("Values[%d]: got %d, want %d", i, got.Values[i], want.Values[i])
			}
		}
	}
	if got.Label != want.Label {
		t.Errorf("Label: got %q, want %q", got.Label, want.Label)
	}
}

func assertWithEnum(t *testing.T, got, want *interop.WithEnum) {
	t.Helper()
	if got.ID != want.ID {
		t.Errorf("ID: got %d, want %d", got.ID, want.ID)
	}
	if got.Color != want.Color {
		t.Errorf("Color: got %d, want %d", got.Color, want.Color)
	}
	if got.Description != want.Description {
		t.Errorf("Description: got %q, want %q", got.Description, want.Description)
	}
}

func floatEq32(a, b float32) bool {
	return math.Float32bits(a) == math.Float32bits(b)
}
