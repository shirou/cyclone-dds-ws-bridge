use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::dds::bindings::*;
use crate::dds::error::DdsError;
use crate::dds::participant::Topic;
use crate::dds::sertype::{free_sample_data, SampleWrapper};
use crate::qos::QosSet;

// Instance state constants (C #defines not picked up by bindgen)
const DDS_IST_ALIVE: u32 = 1;
const DDS_IST_NOT_ALIVE_DISPOSED: u32 = 2;
const DDS_IST_NOT_ALIVE_NO_WRITERS: u32 = 4;

static NEXT_READER_ID: AtomicU64 = AtomicU64::new(1);

/// Data received from DDS, ready to be forwarded to subscribed sessions.
#[derive(Debug, Clone)]
pub struct ReceivedSample {
    pub topic_name: String,
    pub instance_state: InstanceState,
    /// DDS source timestamp in nanoseconds. May be negative (pre-epoch) in rare cases.
    pub source_timestamp: i64,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceState {
    Alive,
    Disposed,
    NoWriters,
}

/// A DDS DataReader.
///
/// DDS entity handles are not thread-safe for concurrent access.
/// All methods on `DdsReader` must be called from the same thread/task,
/// or external synchronization must be used.
pub struct DdsReader {
    pub reader_id: u64,
    pub handle: dds_entity_t,
    pub topic: Topic,
    pub qos_normalized: QosSet,
    /// Sessions subscribed to this reader. The reader is deleted when this is empty.
    pub subscribers: HashSet<u64>,
}

impl DdsReader {
    /// Create a new DataReader on the given topic with optional QoS.
    pub fn new(
        participant_handle: dds_entity_t,
        topic: &Topic,
        qos: *const dds_qos_t,
        qos_normalized: QosSet,
    ) -> Result<Self, DdsError> {
        let handle =
            unsafe { dds_create_reader(participant_handle, topic.handle, qos, ptr::null()) };
        if handle < 0 {
            return Err(DdsError::CreateReader(handle));
        }

        Ok(DdsReader {
            reader_id: NEXT_READER_ID.fetch_add(1, Ordering::Relaxed),
            handle,
            topic: topic.clone(),
            qos_normalized,
            subscribers: HashSet::new(),
        })
    }

    /// Number of sessions currently subscribed to this reader.
    pub fn ref_count(&self) -> usize {
        self.subscribers.len()
    }

    /// Take all available samples from this reader.
    pub fn take_samples(&self, max_samples: usize) -> Result<Vec<ReceivedSample>, DdsError> {
        let mut results = Vec::new();

        for _ in 0..max_samples {
            let mut sample = SampleWrapper {
                data: ptr::null(),
                len: 0,
            };
            let mut sample_ptrs: [*mut c_void; 1] =
                [&mut sample as *mut SampleWrapper as *mut c_void];
            let mut infos: [dds_sample_info_t; 1] = [unsafe { std::mem::zeroed() }];

            let n =
                unsafe { dds_take(self.handle, sample_ptrs.as_mut_ptr(), infos.as_mut_ptr(), 1, 1) };

            if n <= 0 {
                break;
            }

            let info = &infos[0];
            let instance_state = match info.instance_state {
                s if s == DDS_IST_ALIVE => InstanceState::Alive,
                s if s == DDS_IST_NOT_ALIVE_DISPOSED => InstanceState::Disposed,
                s if s == DDS_IST_NOT_ALIVE_NO_WRITERS => InstanceState::NoWriters,
                _ => InstanceState::Alive,
            };

            let should_forward = info.valid_data || instance_state != InstanceState::Alive;

            if should_forward {
                let data = if !sample.data.is_null() && sample.len > 0 {
                    unsafe { std::slice::from_raw_parts(sample.data, sample.len) }.to_vec()
                } else {
                    Vec::new()
                };

                results.push(ReceivedSample {
                    topic_name: self.topic.topic_name.clone(),
                    instance_state,
                    source_timestamp: info.source_timestamp,
                    data,
                });
            }

            unsafe {
                free_sample_data(&mut sample);
            }
        }

        Ok(results)
    }
}

impl Drop for DdsReader {
    fn drop(&mut self) {
        unsafe {
            dds_delete(self.handle);
        }
    }
}

/// Key for sharing readers: (topic_name, type_name, normalized_qos).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReaderKey {
    topic_name: String,
    type_name: String,
    // QoS serialized to bytes for hashing (normalized form)
    qos_bytes: Vec<u8>,
}

impl ReaderKey {
    fn new(topic_name: &str, type_name: &str, qos: &QosSet) -> Self {
        let normalized = qos.normalize();
        let mut buf = bytes::BytesMut::new();
        crate::qos::serialize_qos(&normalized, &mut buf);
        ReaderKey {
            topic_name: topic_name.to_string(),
            type_name: type_name.to_string(),
            qos_bytes: buf.to_vec(),
        }
    }
}

/// Registry of readers, shared by (topic_name, type_name, normalized_qos).
/// Reference-counted via `subscribers.len()`: the DDS reader is deleted when
/// the last subscriber leaves.
pub struct ReaderRegistry {
    /// reader_id -> DdsReader
    readers: HashMap<u64, DdsReader>,
    /// ReaderKey -> reader_id (for sharing)
    key_to_reader: HashMap<ReaderKey, u64>,
    /// (session_id, topic_name, type_name) -> reader_id
    /// Allows unsubscribe without QoS (the protocol only sends topic+type).
    session_subscriptions: HashMap<(u64, String, String), u64>,
}

impl ReaderRegistry {
    pub fn new() -> Self {
        ReaderRegistry {
            readers: HashMap::new(),
            key_to_reader: HashMap::new(),
            session_subscriptions: HashMap::new(),
        }
    }

    /// Subscribe a session to a reader. Creates a new reader if none exists
    /// for the given (topic, type, qos) combination.
    /// Returns the reader_id.
    pub fn subscribe(
        &mut self,
        participant_handle: dds_entity_t,
        topic: &Topic,
        qos: *const dds_qos_t,
        qos_set: &QosSet,
        session_id: u64,
    ) -> Result<u64, DdsError> {
        let key = ReaderKey::new(&topic.topic_name, &topic.type_name, qos_set);

        if let Some(&reader_id) = self.key_to_reader.get(&key) {
            if let Some(reader) = self.readers.get_mut(&reader_id) {
                reader.subscribers.insert(session_id);
                self.session_subscriptions.insert(
                    (session_id, topic.topic_name.clone(), topic.type_name.clone()),
                    reader_id,
                );
                return Ok(reader_id);
            }
        }

        let mut reader =
            DdsReader::new(participant_handle, topic, qos, qos_set.normalize())?;
        let reader_id = reader.reader_id;
        reader.subscribers.insert(session_id);

        self.session_subscriptions.insert(
            (session_id, topic.topic_name.clone(), topic.type_name.clone()),
            reader_id,
        );
        self.key_to_reader.insert(key, reader_id);
        self.readers.insert(reader_id, reader);
        Ok(reader_id)
    }

    /// Unsubscribe a session from a reader by topic_name and type_name.
    /// (The protocol's UNSUBSCRIBE message only carries topic+type, not QoS.)
    /// Returns true if the session was actually subscribed.
    pub fn unsubscribe(
        &mut self,
        topic_name: &str,
        type_name: &str,
        session_id: u64,
    ) -> bool {
        let sub_key = (session_id, topic_name.to_string(), type_name.to_string());
        let reader_id = match self.session_subscriptions.remove(&sub_key) {
            Some(id) => id,
            None => return false,
        };

        let should_remove = if let Some(reader) = self.readers.get_mut(&reader_id) {
            reader.subscribers.remove(&session_id);
            reader.subscribers.is_empty()
        } else {
            return false;
        };

        if should_remove {
            self.readers.remove(&reader_id);
            self.key_to_reader.retain(|_, &mut rid| rid != reader_id);
        }
        true
    }

    /// Remove all subscriptions for a session (cleanup on disconnect).
    pub fn remove_session(&mut self, session_id: u64) {
        // Collect reader_ids this session is subscribed to
        let sub_keys: Vec<_> = self
            .session_subscriptions
            .keys()
            .filter(|(sid, _, _)| *sid == session_id)
            .cloned()
            .collect();

        let mut affected_readers = HashSet::new();
        for key in &sub_keys {
            if let Some(reader_id) = self.session_subscriptions.remove(key) {
                affected_readers.insert(reader_id);
            }
        }

        let mut to_remove = Vec::new();
        for reader_id in affected_readers {
            if let Some(reader) = self.readers.get_mut(&reader_id) {
                reader.subscribers.remove(&session_id);
                if reader.subscribers.is_empty() {
                    to_remove.push(reader_id);
                }
            }
        }

        for reader_id in &to_remove {
            self.readers.remove(reader_id);
        }
        self.key_to_reader
            .retain(|_, &mut rid| !to_remove.contains(&rid));
    }

    /// Get a reader by ID.
    pub fn get(&self, reader_id: u64) -> Option<&DdsReader> {
        self.readers.get(&reader_id)
    }

    /// Get all readers (for polling in the bridge event loop).
    pub fn all_readers(&self) -> impl Iterator<Item = &DdsReader> {
        self.readers.values()
    }

    /// Get subscribers for a given reader.
    pub fn get_subscribers(&self, reader_id: u64) -> Option<&HashSet<u64>> {
        self.readers.get(&reader_id).map(|r| &r.subscribers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::KeyDescriptors;

    fn make_topic(name: &str, type_name: &str) -> Topic {
        Topic {
            handle: 0,
            topic_name: name.to_string(),
            type_name: type_name.to_string(),
            key_descriptors: KeyDescriptors { keys: vec![] },
        }
    }

    fn make_reader(id: u64, topic: Topic, qos: QosSet) -> DdsReader {
        DdsReader {
            reader_id: id,
            handle: 0,
            topic,
            qos_normalized: qos.normalize(),
            subscribers: HashSet::new(),
        }
    }

    #[test]
    fn test_ref_count_is_subscribers_len() {
        let mut reader = make_reader(1, make_topic("t", "T"), QosSet::default());
        assert_eq!(reader.ref_count(), 0);
        reader.subscribers.insert(10);
        assert_eq!(reader.ref_count(), 1);
        reader.subscribers.insert(20);
        assert_eq!(reader.ref_count(), 2);
        // Same session inserted again: no change
        reader.subscribers.insert(20);
        assert_eq!(reader.ref_count(), 2);
    }

    #[test]
    fn test_unsubscribe_without_qos() {
        let mut reg = ReaderRegistry::new();
        let topic = make_topic("t", "T");

        // Manually insert a reader + subscription mapping
        let mut reader = make_reader(1, topic.clone(), QosSet::default());
        reader.subscribers.insert(42);
        let key = ReaderKey::new("t", "T", &QosSet::default());
        reg.readers.insert(1, reader);
        reg.key_to_reader.insert(key, 1);
        reg.session_subscriptions
            .insert((42, "t".to_string(), "T".to_string()), 1);

        // Unsubscribe with only topic+type (no QoS)
        assert!(reg.unsubscribe("t", "T", 42));
        // Reader should be removed (last subscriber)
        assert!(reg.readers.is_empty());
        assert!(reg.key_to_reader.is_empty());
    }

    #[test]
    fn test_unsubscribe_not_found() {
        let mut reg = ReaderRegistry::new();
        assert!(!reg.unsubscribe("t", "T", 42));
    }

    #[test]
    fn test_shared_reader_not_removed_until_last() {
        let mut reg = ReaderRegistry::new();
        let topic = make_topic("t", "T");

        let mut reader = make_reader(1, topic.clone(), QosSet::default());
        reader.subscribers.insert(10);
        reader.subscribers.insert(20);
        let key = ReaderKey::new("t", "T", &QosSet::default());
        reg.readers.insert(1, reader);
        reg.key_to_reader.insert(key, 1);
        reg.session_subscriptions
            .insert((10, "t".to_string(), "T".to_string()), 1);
        reg.session_subscriptions
            .insert((20, "t".to_string(), "T".to_string()), 1);

        // First unsubscribe: reader still alive
        assert!(reg.unsubscribe("t", "T", 10));
        assert_eq!(reg.readers.len(), 1);

        // Second unsubscribe: reader removed
        assert!(reg.unsubscribe("t", "T", 20));
        assert!(reg.readers.is_empty());
    }

    #[test]
    fn test_remove_session() {
        let mut reg = ReaderRegistry::new();
        let topic = make_topic("t", "T");

        let mut reader = make_reader(1, topic.clone(), QosSet::default());
        reader.subscribers.insert(42);
        let key = ReaderKey::new("t", "T", &QosSet::default());
        reg.readers.insert(1, reader);
        reg.key_to_reader.insert(key, 1);
        reg.session_subscriptions
            .insert((42, "t".to_string(), "T".to_string()), 1);

        reg.remove_session(42);
        assert!(reg.readers.is_empty());
        assert!(reg.session_subscriptions.is_empty());
    }
}
