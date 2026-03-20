use crate::dds::bindings::*;
use crate::qos::QosSet;

/// RAII wrapper for a Cyclone DDS QoS handle.
/// Calls `dds_delete_qos` on drop.
pub struct QosHandle {
    ptr: *mut dds_qos_t,
}

impl QosHandle {
    /// Convert a QosSet to a DDS QoS handle.
    /// Returns `None` if the QoS set is empty (callers should pass NULL to DDS).
    pub fn from_qos_set(qos: &QosSet) -> Option<Self> {
        use std::ffi::CString;

        if qos.policies.is_empty() {
            return None;
        }

        unsafe {
            let q = dds_create_qos();
            if q.is_null() {
                return None;
            }

            for policy in &qos.policies {
                match policy {
                    crate::qos::QosPolicy::Reliability {
                        kind,
                        max_blocking_time_ms,
                    } => {
                        let rel_kind = match kind {
                            0 => dds_reliability_kind_DDS_RELIABILITY_BEST_EFFORT,
                            _ => dds_reliability_kind_DDS_RELIABILITY_RELIABLE,
                        };
                        let blocking_ns = (*max_blocking_time_ms as i64) * 1_000_000;
                        dds_qset_reliability(q, rel_kind, blocking_ns);
                    }
                    crate::qos::QosPolicy::Durability { kind } => {
                        let dur_kind = match kind {
                            0 => dds_durability_kind_DDS_DURABILITY_VOLATILE,
                            1 => dds_durability_kind_DDS_DURABILITY_TRANSIENT_LOCAL,
                            2 => dds_durability_kind_DDS_DURABILITY_TRANSIENT,
                            _ => dds_durability_kind_DDS_DURABILITY_PERSISTENT,
                        };
                        dds_qset_durability(q, dur_kind);
                    }
                    crate::qos::QosPolicy::History { kind, depth } => {
                        let hist_kind = match kind {
                            0 => dds_history_kind_DDS_HISTORY_KEEP_LAST,
                            _ => dds_history_kind_DDS_HISTORY_KEEP_ALL,
                        };
                        dds_qset_history(q, hist_kind, *depth as i32);
                    }
                    crate::qos::QosPolicy::Deadline { period_ms } => {
                        let ns = if *period_ms == 0 {
                            i64::MAX
                        } else {
                            (*period_ms as i64) * 1_000_000
                        };
                        dds_qset_deadline(q, ns);
                    }
                    crate::qos::QosPolicy::LatencyBudget { duration_ms } => {
                        let ns = (*duration_ms as i64) * 1_000_000;
                        dds_qset_latency_budget(q, ns);
                    }
                    crate::qos::QosPolicy::OwnershipStrength { value } => {
                        dds_qset_ownership_strength(q, *value as i32);
                    }
                    crate::qos::QosPolicy::Liveliness {
                        kind,
                        lease_duration_ms,
                    } => {
                        let live_kind = match kind {
                            0 => dds_liveliness_kind_DDS_LIVELINESS_AUTOMATIC,
                            1 => dds_liveliness_kind_DDS_LIVELINESS_MANUAL_BY_PARTICIPANT,
                            _ => dds_liveliness_kind_DDS_LIVELINESS_MANUAL_BY_TOPIC,
                        };
                        let ns = if *lease_duration_ms == 0 {
                            i64::MAX
                        } else {
                            (*lease_duration_ms as i64) * 1_000_000
                        };
                        dds_qset_liveliness(q, live_kind, ns);
                    }
                    crate::qos::QosPolicy::DestinationOrder { kind } => {
                        let ord_kind = match kind {
                            0 => dds_destination_order_kind_DDS_DESTINATIONORDER_BY_RECEPTION_TIMESTAMP,
                            _ => dds_destination_order_kind_DDS_DESTINATIONORDER_BY_SOURCE_TIMESTAMP,
                        };
                        dds_qset_destination_order(q, ord_kind);
                    }
                    crate::qos::QosPolicy::Presentation {
                        access_scope,
                        coherent_access,
                        ordered_access,
                    } => {
                        let scope = match access_scope {
                            0 => dds_presentation_access_scope_kind_DDS_PRESENTATION_INSTANCE,
                            1 => dds_presentation_access_scope_kind_DDS_PRESENTATION_TOPIC,
                            _ => dds_presentation_access_scope_kind_DDS_PRESENTATION_GROUP,
                        };
                        dds_qset_presentation(q, scope, *coherent_access != 0, *ordered_access != 0);
                    }
                    crate::qos::QosPolicy::Partition { partitions } => {
                        let c_strings: Vec<CString> = partitions
                            .iter()
                            .map(|s| CString::new(s.as_str()).unwrap_or_default())
                            .collect();
                        let mut c_ptrs: Vec<*const std::os::raw::c_char> =
                            c_strings.iter().map(|s| s.as_ptr()).collect();
                        dds_qset_partition(q, c_ptrs.len() as u32, c_ptrs.as_mut_ptr());
                    }
                    crate::qos::QosPolicy::Ownership { kind } => {
                        let own_kind = match kind {
                            0 => dds_ownership_kind_DDS_OWNERSHIP_SHARED,
                            _ => dds_ownership_kind_DDS_OWNERSHIP_EXCLUSIVE,
                        };
                        dds_qset_ownership(q, own_kind);
                    }
                    crate::qos::QosPolicy::WriterDataLifecycle {
                        autodispose_unregistered_instances,
                    } => {
                        dds_qset_writer_data_lifecycle(q, *autodispose_unregistered_instances != 0);
                    }
                    crate::qos::QosPolicy::TimeBasedFilter {
                        minimum_separation_ms,
                    } => {
                        let ns = (*minimum_separation_ms as i64) * 1_000_000;
                        dds_qset_time_based_filter(q, ns);
                    }
                }
            }

            Some(QosHandle { ptr: q })
        }
    }

    /// Get the raw pointer for passing to DDS API functions.
    pub fn as_ptr(&self) -> *const dds_qos_t {
        self.ptr as *const _
    }
}

impl Drop for QosHandle {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                dds_delete_qos(self.ptr);
            }
        }
    }
}

/// Helper: get a QoS raw pointer or null for empty QoS.
/// The returned QosHandle (if Some) must be kept alive while the pointer is in use.
pub fn make_qos(qos: &QosSet) -> (Option<QosHandle>, *const dds_qos_t) {
    match QosHandle::from_qos_set(qos) {
        Some(handle) => {
            let ptr = handle.as_ptr();
            (Some(handle), ptr)
        }
        None => (None, std::ptr::null()),
    }
}
