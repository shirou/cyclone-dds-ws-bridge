use bytes::{Buf, BufMut, BytesMut};

use crate::protocol::ProtocolError;

fn validate_range(name: &str, value: u8, max: u8) -> Result<(), ProtocolError> {
    if value > max {
        Err(ProtocolError::InvalidQosValue(format!(
            "{name}: {value} not in 0..={max}"
        )))
    } else {
        Ok(())
    }
}

fn validate_bool(name: &str, value: u8) -> Result<(), ProtocolError> {
    if value > 1 {
        Err(ProtocolError::InvalidQosValue(format!(
            "{name}: expected 0 or 1, got {value}"
        )))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum QosPolicy {
    Reliability {
        kind: u8, // 0=BEST_EFFORT, 1=RELIABLE
        max_blocking_time_ms: u32,
    },
    Durability {
        kind: u8, // 0=VOLATILE, 1=TRANSIENT_LOCAL, 2=TRANSIENT, 3=PERSISTENT
    },
    History {
        kind: u8, // 0=KEEP_LAST, 1=KEEP_ALL
        depth: u32,
    },
    Deadline {
        period_ms: u32, // 0 = infinite
    },
    LatencyBudget {
        duration_ms: u32,
    },
    OwnershipStrength {
        value: u32,
    },
    Liveliness {
        kind: u8, // 0=AUTOMATIC, 1=MANUAL_BY_PARTICIPANT, 2=MANUAL_BY_TOPIC
        lease_duration_ms: u32,
    },
    DestinationOrder {
        kind: u8, // 0=BY_RECEPTION_TIMESTAMP, 1=BY_SOURCE_TIMESTAMP
    },
    Presentation {
        access_scope: u8, // 0=INSTANCE, 1=TOPIC, 2=GROUP
        coherent_access: u8, // 0 or 1
        ordered_access: u8, // 0 or 1
    },
    Partition {
        partitions: Vec<String>,
    },
    Ownership {
        kind: u8, // 0=SHARED, 1=EXCLUSIVE
    },
    WriterDataLifecycle {
        autodispose_unregistered_instances: u8, // 0 or 1
    },
    TimeBasedFilter {
        minimum_separation_ms: u32,
    },
}

impl QosPolicy {
    pub fn policy_id(&self) -> u8 {
        match self {
            Self::Reliability { .. } => 0x01,
            Self::Durability { .. } => 0x02,
            Self::History { .. } => 0x03,
            Self::Deadline { .. } => 0x04,
            Self::LatencyBudget { .. } => 0x05,
            Self::OwnershipStrength { .. } => 0x06,
            Self::Liveliness { .. } => 0x07,
            Self::DestinationOrder { .. } => 0x08,
            Self::Presentation { .. } => 0x09,
            Self::Partition { .. } => 0x0A,
            Self::Ownership { .. } => 0x0B,
            Self::WriterDataLifecycle { .. } => 0x0C,
            Self::TimeBasedFilter { .. } => 0x0D,
        }
    }

    fn is_default(&self) -> bool {
        match self {
            Self::Reliability {
                kind,
                max_blocking_time_ms,
            } => *kind == 1 && *max_blocking_time_ms == 100,
            Self::Durability { kind } => *kind == 0,
            Self::History { kind, depth } => *kind == 0 && *depth == 1,
            Self::Deadline { period_ms } => *period_ms == 0,
            Self::LatencyBudget { duration_ms } => *duration_ms == 0,
            Self::OwnershipStrength { value } => *value == 0,
            Self::Liveliness {
                kind,
                lease_duration_ms,
            } => *kind == 0 && *lease_duration_ms == 0,
            Self::DestinationOrder { kind } => *kind == 0,
            Self::Presentation {
                access_scope,
                coherent_access,
                ordered_access,
            } => *access_scope == 0 && *coherent_access == 0 && *ordered_access == 0,
            Self::Partition { partitions } => partitions.is_empty(),
            Self::Ownership { kind } => *kind == 0,
            Self::WriterDataLifecycle {
                autodispose_unregistered_instances,
            } => *autodispose_unregistered_instances == 1,
            Self::TimeBasedFilter {
                minimum_separation_ms,
            } => *minimum_separation_ms == 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct QosSet {
    pub policies: Vec<QosPolicy>,
}

impl QosSet {
    /// Normalize: remove policies that are set to their default values.
    pub fn normalize(&self) -> QosSet {
        QosSet {
            policies: self
                .policies
                .iter()
                .filter(|p| !p.is_default())
                .cloned()
                .collect(),
        }
    }

    /// Normalized equality: two QosSets are equal if they have the same
    /// non-default policies (order-independent).
    pub fn normalized_eq(&self, other: &QosSet) -> bool {
        let a = self.normalize();
        let b = other.normalize();
        if a.policies.len() != b.policies.len() {
            return false;
        }
        for pa in &a.policies {
            if !b.policies.iter().any(|pb| pa == pb) {
                return false;
            }
        }
        true
    }
}

pub fn parse_qos(buf: &mut &[u8]) -> Result<QosSet, ProtocolError> {
    if buf.remaining() < 1 {
        return Err(ProtocolError::Truncated {
            expected: 1,
            got: 0,
        });
    }
    let policy_count = buf.get_u8() as usize;
    let mut policies = Vec::with_capacity(policy_count);
    let mut seen_ids: u16 = 0; // bitmask for policy_ids 0x01..0x0D

    for _ in 0..policy_count {
        if buf.remaining() < 1 {
            return Err(ProtocolError::Truncated {
                expected: 1,
                got: 0,
            });
        }
        let policy_id = buf.get_u8();

        // Check for duplicate policy_id
        if policy_id >= 0x01 && policy_id <= 0x0D {
            let bit = 1u16 << policy_id;
            if seen_ids & bit != 0 {
                return Err(ProtocolError::DuplicateQosPolicy(policy_id));
            }
            seen_ids |= bit;
        }

        let policy = match policy_id {
            0x01 => {
                if buf.remaining() < 5 {
                    return Err(ProtocolError::Truncated {
                        expected: 5,
                        got: buf.remaining(),
                    });
                }
                let kind = buf.get_u8();
                validate_range("reliability kind", kind, 1)?;
                let max_blocking_time_ms = buf.get_u32_le();
                QosPolicy::Reliability {
                    kind,
                    max_blocking_time_ms,
                }
            }
            0x02 => {
                if buf.remaining() < 1 {
                    return Err(ProtocolError::Truncated {
                        expected: 1,
                        got: 0,
                    });
                }
                let kind = buf.get_u8();
                validate_range("durability kind", kind, 3)?;
                QosPolicy::Durability { kind }
            }
            0x03 => {
                if buf.remaining() < 5 {
                    return Err(ProtocolError::Truncated {
                        expected: 5,
                        got: buf.remaining(),
                    });
                }
                let kind = buf.get_u8();
                validate_range("history kind", kind, 1)?;
                let depth = buf.get_u32_le();
                QosPolicy::History { kind, depth }
            }
            0x04 => {
                if buf.remaining() < 4 {
                    return Err(ProtocolError::Truncated {
                        expected: 4,
                        got: buf.remaining(),
                    });
                }
                QosPolicy::Deadline {
                    period_ms: buf.get_u32_le(),
                }
            }
            0x05 => {
                if buf.remaining() < 4 {
                    return Err(ProtocolError::Truncated {
                        expected: 4,
                        got: buf.remaining(),
                    });
                }
                QosPolicy::LatencyBudget {
                    duration_ms: buf.get_u32_le(),
                }
            }
            0x06 => {
                if buf.remaining() < 4 {
                    return Err(ProtocolError::Truncated {
                        expected: 4,
                        got: buf.remaining(),
                    });
                }
                QosPolicy::OwnershipStrength {
                    value: buf.get_u32_le(),
                }
            }
            0x07 => {
                if buf.remaining() < 5 {
                    return Err(ProtocolError::Truncated {
                        expected: 5,
                        got: buf.remaining(),
                    });
                }
                let kind = buf.get_u8();
                validate_range("liveliness kind", kind, 2)?;
                let lease_duration_ms = buf.get_u32_le();
                QosPolicy::Liveliness {
                    kind,
                    lease_duration_ms,
                }
            }
            0x08 => {
                if buf.remaining() < 1 {
                    return Err(ProtocolError::Truncated {
                        expected: 1,
                        got: 0,
                    });
                }
                let kind = buf.get_u8();
                validate_range("destination_order kind", kind, 1)?;
                QosPolicy::DestinationOrder { kind }
            }
            0x09 => {
                if buf.remaining() < 3 {
                    return Err(ProtocolError::Truncated {
                        expected: 3,
                        got: buf.remaining(),
                    });
                }
                let access_scope = buf.get_u8();
                let coherent_access = buf.get_u8();
                let ordered_access = buf.get_u8();
                validate_range("presentation access_scope", access_scope, 2)?;
                validate_bool("presentation coherent_access", coherent_access)?;
                validate_bool("presentation ordered_access", ordered_access)?;
                QosPolicy::Presentation {
                    access_scope,
                    coherent_access,
                    ordered_access,
                }
            }
            0x0A => {
                if buf.remaining() < 1 {
                    return Err(ProtocolError::Truncated {
                        expected: 1,
                        got: 0,
                    });
                }
                let partition_count = buf.get_u8() as usize;
                let mut partitions = Vec::with_capacity(partition_count);
                for _ in 0..partition_count {
                    partitions.push(crate::protocol::read_string(buf)?);
                }
                QosPolicy::Partition { partitions }
            }
            0x0B => {
                if buf.remaining() < 1 {
                    return Err(ProtocolError::Truncated {
                        expected: 1,
                        got: 0,
                    });
                }
                let kind = buf.get_u8();
                validate_range("ownership kind", kind, 1)?;
                QosPolicy::Ownership { kind }
            }
            0x0C => {
                if buf.remaining() < 1 {
                    return Err(ProtocolError::Truncated {
                        expected: 1,
                        got: 0,
                    });
                }
                let v = buf.get_u8();
                validate_bool("autodispose_unregistered_instances", v)?;
                QosPolicy::WriterDataLifecycle {
                    autodispose_unregistered_instances: v,
                }
            }
            0x0D => {
                if buf.remaining() < 4 {
                    return Err(ProtocolError::Truncated {
                        expected: 4,
                        got: buf.remaining(),
                    });
                }
                QosPolicy::TimeBasedFilter {
                    minimum_separation_ms: buf.get_u32_le(),
                }
            }
            _ => return Err(ProtocolError::InvalidQosPolicyId(policy_id)),
        };
        policies.push(policy);
    }

    Ok(QosSet { policies })
}

pub fn serialize_qos(qos: &QosSet, buf: &mut BytesMut) {
    buf.put_u8(qos.policies.len() as u8);
    for policy in &qos.policies {
        buf.put_u8(policy.policy_id());
        match policy {
            QosPolicy::Reliability {
                kind,
                max_blocking_time_ms,
            } => {
                buf.put_u8(*kind);
                buf.put_u32_le(*max_blocking_time_ms);
            }
            QosPolicy::Durability { kind } => {
                buf.put_u8(*kind);
            }
            QosPolicy::History { kind, depth } => {
                buf.put_u8(*kind);
                buf.put_u32_le(*depth);
            }
            QosPolicy::Deadline { period_ms } => {
                buf.put_u32_le(*period_ms);
            }
            QosPolicy::LatencyBudget { duration_ms } => {
                buf.put_u32_le(*duration_ms);
            }
            QosPolicy::OwnershipStrength { value } => {
                buf.put_u32_le(*value);
            }
            QosPolicy::Liveliness {
                kind,
                lease_duration_ms,
            } => {
                buf.put_u8(*kind);
                buf.put_u32_le(*lease_duration_ms);
            }
            QosPolicy::DestinationOrder { kind } => {
                buf.put_u8(*kind);
            }
            QosPolicy::Presentation {
                access_scope,
                coherent_access,
                ordered_access,
            } => {
                buf.put_u8(*access_scope);
                buf.put_u8(*coherent_access);
                buf.put_u8(*ordered_access);
            }
            QosPolicy::Partition { partitions } => {
                buf.put_u8(partitions.len() as u8);
                for p in partitions {
                    crate::protocol::write_string(buf, p);
                }
            }
            QosPolicy::Ownership { kind } => {
                buf.put_u8(*kind);
            }
            QosPolicy::WriterDataLifecycle {
                autodispose_unregistered_instances,
            } => {
                buf.put_u8(*autodispose_unregistered_instances);
            }
            QosPolicy::TimeBasedFilter {
                minimum_separation_ms,
            } => {
                buf.put_u32_le(*minimum_separation_ms);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qos_round_trip(qos: &QosSet) -> QosSet {
        let mut buf = BytesMut::new();
        serialize_qos(qos, &mut buf);
        let bytes = buf.freeze();
        let mut slice: &[u8] = &bytes;
        parse_qos(&mut slice).unwrap()
    }

    #[test]
    fn test_empty_qos() {
        let qos = QosSet::default();
        let parsed = qos_round_trip(&qos);
        assert_eq!(parsed.policies.len(), 0);
    }

    #[test]
    fn test_reliability() {
        let qos = QosSet {
            policies: vec![QosPolicy::Reliability {
                kind: 1,
                max_blocking_time_ms: 100,
            }],
        };
        assert_eq!(qos, qos_round_trip(&qos));
    }

    #[test]
    fn test_durability() {
        let qos = QosSet {
            policies: vec![QosPolicy::Durability { kind: 1 }],
        };
        assert_eq!(qos, qos_round_trip(&qos));
    }

    #[test]
    fn test_history() {
        let qos = QosSet {
            policies: vec![QosPolicy::History { kind: 0, depth: 10 }],
        };
        assert_eq!(qos, qos_round_trip(&qos));
    }

    #[test]
    fn test_deadline() {
        let qos = QosSet {
            policies: vec![QosPolicy::Deadline { period_ms: 500 }],
        };
        assert_eq!(qos, qos_round_trip(&qos));
    }

    #[test]
    fn test_latency_budget() {
        let qos = QosSet {
            policies: vec![QosPolicy::LatencyBudget { duration_ms: 50 }],
        };
        assert_eq!(qos, qos_round_trip(&qos));
    }

    #[test]
    fn test_ownership_strength() {
        let qos = QosSet {
            policies: vec![QosPolicy::OwnershipStrength { value: 42 }],
        };
        assert_eq!(qos, qos_round_trip(&qos));
    }

    #[test]
    fn test_liveliness() {
        let qos = QosSet {
            policies: vec![QosPolicy::Liveliness {
                kind: 2,
                lease_duration_ms: 1000,
            }],
        };
        assert_eq!(qos, qos_round_trip(&qos));
    }

    #[test]
    fn test_destination_order() {
        let qos = QosSet {
            policies: vec![QosPolicy::DestinationOrder { kind: 1 }],
        };
        assert_eq!(qos, qos_round_trip(&qos));
    }

    #[test]
    fn test_presentation() {
        let qos = QosSet {
            policies: vec![QosPolicy::Presentation {
                access_scope: 1,
                coherent_access: 1,
                ordered_access: 0,
            }],
        };
        assert_eq!(qos, qos_round_trip(&qos));
    }

    #[test]
    fn test_partition() {
        let qos = QosSet {
            policies: vec![QosPolicy::Partition {
                partitions: vec!["part_a".into(), "part_b".into()],
            }],
        };
        assert_eq!(qos, qos_round_trip(&qos));
    }

    #[test]
    fn test_partition_empty() {
        let qos = QosSet {
            policies: vec![QosPolicy::Partition {
                partitions: vec![],
            }],
        };
        assert_eq!(qos, qos_round_trip(&qos));
    }

    #[test]
    fn test_ownership() {
        let qos = QosSet {
            policies: vec![QosPolicy::Ownership { kind: 1 }],
        };
        assert_eq!(qos, qos_round_trip(&qos));
    }

    #[test]
    fn test_writer_data_lifecycle() {
        let qos = QosSet {
            policies: vec![QosPolicy::WriterDataLifecycle {
                autodispose_unregistered_instances: 0,
            }],
        };
        assert_eq!(qos, qos_round_trip(&qos));
    }

    #[test]
    fn test_time_based_filter() {
        let qos = QosSet {
            policies: vec![QosPolicy::TimeBasedFilter {
                minimum_separation_ms: 100,
            }],
        };
        assert_eq!(qos, qos_round_trip(&qos));
    }

    #[test]
    fn test_multiple_policies() {
        let qos = QosSet {
            policies: vec![
                QosPolicy::Reliability {
                    kind: 1,
                    max_blocking_time_ms: 100,
                },
                QosPolicy::Durability { kind: 1 },
                QosPolicy::History {
                    kind: 0,
                    depth: 10,
                },
            ],
        };
        assert_eq!(qos, qos_round_trip(&qos));
    }

    #[test]
    fn test_invalid_policy_id() {
        let data = [1u8, 0xFF]; // 1 policy, invalid id 0xFF
        let mut slice: &[u8] = &data;
        let err = parse_qos(&mut slice).unwrap_err();
        assert!(matches!(err, ProtocolError::InvalidQosPolicyId(0xFF)));
    }

    #[test]
    fn test_duplicate_policy_rejected() {
        // Two Reliability policies
        let mut buf = BytesMut::new();
        buf.put_u8(2); // policy_count
        // First Reliability
        buf.put_u8(0x01);
        buf.put_u8(1); // kind=RELIABLE
        buf.put_u32_le(100);
        // Second Reliability (duplicate)
        buf.put_u8(0x01);
        buf.put_u8(0); // kind=BEST_EFFORT
        buf.put_u32_le(0);

        let bytes = buf.freeze();
        let mut slice: &[u8] = &bytes;
        let err = parse_qos(&mut slice).unwrap_err();
        assert!(matches!(err, ProtocolError::DuplicateQosPolicy(0x01)));
    }

    #[test]
    fn test_invalid_reliability_kind() {
        let mut buf = BytesMut::new();
        buf.put_u8(1);
        buf.put_u8(0x01); // Reliability
        buf.put_u8(2); // invalid kind (must be 0 or 1)
        buf.put_u32_le(100);

        let bytes = buf.freeze();
        let mut slice: &[u8] = &bytes;
        let err = parse_qos(&mut slice).unwrap_err();
        assert!(matches!(err, ProtocolError::InvalidQosValue(_)));
    }

    #[test]
    fn test_invalid_durability_kind() {
        let mut buf = BytesMut::new();
        buf.put_u8(1);
        buf.put_u8(0x02); // Durability
        buf.put_u8(4); // invalid (must be 0..=3)

        let bytes = buf.freeze();
        let mut slice: &[u8] = &bytes;
        assert!(matches!(
            parse_qos(&mut slice),
            Err(ProtocolError::InvalidQosValue(_))
        ));
    }

    #[test]
    fn test_invalid_bool_field() {
        let mut buf = BytesMut::new();
        buf.put_u8(1);
        buf.put_u8(0x0C); // WriterDataLifecycle
        buf.put_u8(2); // invalid bool (must be 0 or 1)

        let bytes = buf.freeze();
        let mut slice: &[u8] = &bytes;
        assert!(matches!(
            parse_qos(&mut slice),
            Err(ProtocolError::InvalidQosValue(_))
        ));
    }

    #[test]
    fn test_invalid_presentation_coherent_access() {
        let mut buf = BytesMut::new();
        buf.put_u8(1);
        buf.put_u8(0x09); // Presentation
        buf.put_u8(0); // access_scope OK
        buf.put_u8(5); // coherent_access invalid
        buf.put_u8(0); // ordered_access OK

        let bytes = buf.freeze();
        let mut slice: &[u8] = &bytes;
        assert!(matches!(
            parse_qos(&mut slice),
            Err(ProtocolError::InvalidQosValue(_))
        ));
    }

    #[test]
    fn test_normalized_eq_explicit_defaults_vs_omitted() {
        let explicit = QosSet {
            policies: vec![QosPolicy::Reliability {
                kind: 1,
                max_blocking_time_ms: 100,
            }],
        };
        let implicit = QosSet::default();
        assert!(explicit.normalized_eq(&implicit));
    }

    #[test]
    fn test_normalized_eq_different() {
        let a = QosSet {
            policies: vec![QosPolicy::Durability { kind: 1 }],
        };
        let b = QosSet {
            policies: vec![QosPolicy::Durability { kind: 0 }],
        };
        assert!(!a.normalized_eq(&b));
    }

    #[test]
    fn test_normalized_eq_order_independent() {
        let a = QosSet {
            policies: vec![
                QosPolicy::Durability { kind: 1 },
                QosPolicy::History {
                    kind: 0,
                    depth: 10,
                },
            ],
        };
        let b = QosSet {
            policies: vec![
                QosPolicy::History {
                    kind: 0,
                    depth: 10,
                },
                QosPolicy::Durability { kind: 1 },
            ],
        };
        assert!(a.normalized_eq(&b));
    }

    #[test]
    fn test_normalize_removes_defaults() {
        let qos = QosSet {
            policies: vec![
                QosPolicy::Reliability {
                    kind: 1,
                    max_blocking_time_ms: 100,
                },
                QosPolicy::Durability { kind: 0 },
                QosPolicy::History { kind: 0, depth: 1 },
                QosPolicy::Ownership { kind: 0 },
            ],
        };
        let normalized = qos.normalize();
        assert!(normalized.policies.is_empty());
    }

    #[test]
    fn test_normalize_keeps_non_defaults() {
        let qos = QosSet {
            policies: vec![
                QosPolicy::Reliability {
                    kind: 0, // BEST_EFFORT (non-default)
                    max_blocking_time_ms: 100,
                },
                QosPolicy::Durability { kind: 1 }, // TRANSIENT_LOCAL (non-default)
            ],
        };
        let normalized = qos.normalize();
        assert_eq!(normalized.policies.len(), 2);
    }
}
