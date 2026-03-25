use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::dds::bindings::*;
use crate::dds::error::DdsError;
use crate::dds::participant::Topic;
use crate::dds::sertype::SampleWrapper;
use crate::qos::QosSet;

/// Global atomic counter for generating unique writer IDs.
static NEXT_WRITER_ID: AtomicU32 = AtomicU32::new(1);

fn next_writer_id() -> u32 {
    NEXT_WRITER_ID.fetch_add(1, Ordering::Relaxed)
}

/// A DDS DataWriter handle.
///
/// DDS entity handles are not thread-safe for concurrent access.
/// All methods on `DdsWriter` must be called from the same thread/task,
/// or external synchronization must be used.
pub struct DdsWriter {
    pub handle: dds_entity_t,
    pub writer_id: u32,
    pub topic: Topic,
    /// Normalized QoS used to create this writer.
    pub qos_normalized: QosSet,
}

impl DdsWriter {
    /// Create a new DataWriter on the given topic with optional QoS.
    pub fn new(
        participant_handle: dds_entity_t,
        topic: &Topic,
        qos: *const dds_qos_t,
        qos_normalized: QosSet,
    ) -> Result<Self, DdsError> {
        let handle =
            unsafe { dds_create_writer(participant_handle, topic.handle, qos, ptr::null()) };
        if handle < 0 {
            return Err(DdsError::CreateWriter(handle));
        }
        Ok(DdsWriter {
            handle,
            writer_id: next_writer_id(),
            topic: topic.clone(),
            qos_normalized,
        })
    }

    /// Write opaque data.
    pub fn write(&self, data: &[u8]) -> Result<(), DdsError> {
        // Safety: SampleWrapper borrows `data` which lives for the duration of dds_write.
        let sample = unsafe { SampleWrapper::from_bytes(data) };
        let rc =
            unsafe { dds_write(self.handle, &sample as *const SampleWrapper as *const c_void) };
        if rc < 0 {
            return Err(DdsError::Write(rc));
        }
        Ok(())
    }

    /// Dispose an instance (key data only).
    pub fn dispose(&self, key_data: &[u8]) -> Result<(), DdsError> {
        let sample = unsafe { SampleWrapper::from_bytes(key_data) };
        let rc =
            unsafe { dds_dispose(self.handle, &sample as *const SampleWrapper as *const c_void) };
        if rc < 0 {
            return Err(DdsError::Dispose(rc));
        }
        Ok(())
    }

    /// Write and dispose in one operation.
    pub fn write_dispose(&self, data: &[u8]) -> Result<(), DdsError> {
        let sample = unsafe { SampleWrapper::from_bytes(data) };
        let rc = unsafe {
            dds_writedispose(self.handle, &sample as *const SampleWrapper as *const c_void)
        };
        if rc < 0 {
            return Err(DdsError::WriteDispose(rc));
        }
        Ok(())
    }
}

impl Drop for DdsWriter {
    fn drop(&mut self) {
        unsafe {
            dds_delete(self.handle);
        }
    }
}

/// Registry of writers, keyed by writer_id.
/// Tracks ownership: each writer belongs to a session.
pub struct WriterRegistry {
    /// writer_id -> (DdsWriter, session_id)
    writers: HashMap<u32, (DdsWriter, u64)>,
    /// session_id -> list of writer_ids (both explicit and implicit)
    session_writers: HashMap<u64, Vec<u32>>,
}

impl WriterRegistry {
    pub fn new() -> Self {
        WriterRegistry {
            writers: HashMap::new(),
            session_writers: HashMap::new(),
        }
    }

    /// Register a writer for a session. Returns the writer_id.
    pub fn register(&mut self, writer: DdsWriter, session_id: u64) -> u32 {
        let wid = writer.writer_id;
        self.session_writers
            .entry(session_id)
            .or_default()
            .push(wid);
        self.writers.insert(wid, (writer, session_id));
        wid
    }

    /// Get a writer by ID without ownership check (for topic lookup).
    pub fn find_by_id(&self, writer_id: u32) -> Option<&DdsWriter> {
        self.writers.get(&writer_id).map(|(w, _)| w)
    }

    /// Get a writer by ID, checking session ownership.
    pub fn get(&self, writer_id: u32, session_id: u64) -> Result<&DdsWriter, DdsError> {
        match self.writers.get(&writer_id) {
            None => Err(DdsError::WriterNotFound(writer_id)),
            Some((writer, owner)) => {
                if *owner != session_id {
                    Err(DdsError::WriterOwnership(writer_id))
                } else {
                    Ok(writer)
                }
            }
        }
    }

    /// Delete a writer by ID, checking session ownership.
    pub fn delete(&mut self, writer_id: u32, session_id: u64) -> Result<(), DdsError> {
        match self.writers.get(&writer_id) {
            None => return Err(DdsError::WriterNotFound(writer_id)),
            Some((_, owner)) => {
                if *owner != session_id {
                    return Err(DdsError::WriterOwnership(writer_id));
                }
            }
        }
        self.writers.remove(&writer_id);
        if let Some(ids) = self.session_writers.get_mut(&session_id) {
            ids.retain(|&id| id != writer_id);
        }
        Ok(())
    }

    /// Remove all writers belonging to a session (for cleanup on disconnect).
    pub fn remove_session(&mut self, session_id: u64) {
        if let Some(ids) = self.session_writers.remove(&session_id) {
            for wid in ids {
                self.writers.remove(&wid);
            }
        }
    }

    /// Number of registered writers.
    pub fn count(&self) -> usize {
        self.writers.len()
    }

    /// Find a writer for a session by topic/type/qos (for implicit writer reuse in topic-name mode).
    pub fn find_by_topic(
        &self,
        session_id: u64,
        topic_name: &str,
        type_name: &str,
        qos: &QosSet,
    ) -> Option<&DdsWriter> {
        let ids = self.session_writers.get(&session_id)?;
        for &wid in ids {
            if let Some((writer, _)) = self.writers.get(&wid) {
                if writer.topic.topic_name == topic_name
                    && writer.topic.type_name == type_name
                    && writer.qos_normalized.normalized_eq(qos)
                {
                    return Some(writer);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::KeyDescriptors;
    use crate::qos::QosPolicy;

    fn make_topic(name: &str, type_name: &str) -> Topic {
        Topic {
            handle: 0, // dummy
            topic_name: name.to_string(),
            type_name: type_name.to_string(),
            key_descriptors: KeyDescriptors { keys: vec![] },
        }
    }

    fn make_writer(id: u32, topic: Topic, qos: QosSet) -> DdsWriter {
        DdsWriter {
            handle: 0, // dummy, won't call dds_delete in tests
            writer_id: id,
            topic,
            qos_normalized: qos.normalize(),
        }
    }

    #[test]
    fn test_register_and_get() {
        let mut reg = WriterRegistry::new();
        let writer = make_writer(100, make_topic("t", "T"), QosSet::default());
        reg.register(writer, 1);
        assert!(reg.get(100, 1).is_ok());
    }

    #[test]
    fn test_ownership_check() {
        let mut reg = WriterRegistry::new();
        let writer = make_writer(100, make_topic("t", "T"), QosSet::default());
        reg.register(writer, 1);
        assert!(matches!(reg.get(100, 2), Err(DdsError::WriterOwnership(100))));
    }

    #[test]
    fn test_not_found() {
        let reg = WriterRegistry::new();
        assert!(matches!(reg.get(999, 1), Err(DdsError::WriterNotFound(999))));
    }

    #[test]
    fn test_delete() {
        let mut reg = WriterRegistry::new();
        let writer = make_writer(100, make_topic("t", "T"), QosSet::default());
        reg.register(writer, 1);
        assert!(reg.delete(100, 1).is_ok());
        assert!(matches!(reg.get(100, 1), Err(DdsError::WriterNotFound(100))));
    }

    #[test]
    fn test_delete_ownership_check() {
        let mut reg = WriterRegistry::new();
        let writer = make_writer(100, make_topic("t", "T"), QosSet::default());
        reg.register(writer, 1);
        assert!(matches!(reg.delete(100, 2), Err(DdsError::WriterOwnership(100))));
    }

    #[test]
    fn test_remove_session() {
        let mut reg = WriterRegistry::new();
        let w1 = make_writer(100, make_topic("t1", "T"), QosSet::default());
        let w2 = make_writer(101, make_topic("t2", "T"), QosSet::default());
        reg.register(w1, 1);
        reg.register(w2, 1);
        reg.remove_session(1);
        assert!(matches!(reg.get(100, 1), Err(DdsError::WriterNotFound(100))));
        assert!(matches!(reg.get(101, 1), Err(DdsError::WriterNotFound(101))));
    }

    #[test]
    fn test_find_by_topic_matches_qos() {
        let mut reg = WriterRegistry::new();
        let qos_a = QosSet {
            policies: vec![QosPolicy::Durability { kind: 1 }],
        };
        let qos_b = QosSet {
            policies: vec![QosPolicy::Durability { kind: 2 }],
        };
        let w = make_writer(100, make_topic("t", "T"), qos_a.clone());
        reg.register(w, 1);

        // Same QoS => found
        assert!(reg.find_by_topic(1, "t", "T", &qos_a).is_some());
        // Different QoS => not found
        assert!(reg.find_by_topic(1, "t", "T", &qos_b).is_none());
    }

    #[test]
    fn test_find_by_topic_wrong_session() {
        let mut reg = WriterRegistry::new();
        let w = make_writer(100, make_topic("t", "T"), QosSet::default());
        reg.register(w, 1);
        assert!(reg.find_by_topic(2, "t", "T", &QosSet::default()).is_none());
    }
}
