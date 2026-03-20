use std::collections::HashMap;
use std::ffi::CString;
use std::ptr;

use crate::dds::bindings::*;
use crate::dds::error::DdsError;
use crate::dds::sertype::{create_opaque_sertype, DDS_DOMAIN_DEFAULT};
use crate::protocol::KeyDescriptors;

/// A Cyclone DDS topic handle with metadata.
#[derive(Clone)]
pub struct Topic {
    pub handle: dds_entity_t,
    pub topic_name: String,
    pub type_name: String,
    pub key_descriptors: KeyDescriptors,
}

/// Manages a single DDS DomainParticipant and caches topics.
pub struct DdsParticipant {
    pub handle: dds_entity_t,
    /// Cached topics by (topic_name, type_name).
    topics: HashMap<(String, String), Topic>,
}

impl DdsParticipant {
    /// Create a new DomainParticipant.
    ///
    /// `domain_id` uses `DDS_DOMAIN_DEFAULT` (0xFFFFFFFF) for the default domain.
    /// `config_uri` is set via `CYCLONEDDS_URI` env var (handled by Cyclone DDS itself).
    pub fn new(domain_id: Option<u32>) -> Result<Self, DdsError> {
        let did = domain_id.unwrap_or(DDS_DOMAIN_DEFAULT);
        let dp = unsafe { dds_create_participant(did, ptr::null(), ptr::null()) };
        if dp < 0 {
            return Err(DdsError::CreateParticipant(dp));
        }
        Ok(DdsParticipant {
            handle: dp,
            topics: HashMap::new(),
        })
    }

    /// Get or create a topic. Topics with the same (topic_name, type_name) are reused.
    ///
    /// If a topic with the same name/type already exists but with different key
    /// descriptors, the cached version is returned and the new key_descriptors
    /// are ignored (Cyclone DDS requires a single sertype per topic).
    /// Callers should ensure consistent key descriptors for the same topic.
    pub fn get_or_create_topic(
        &mut self,
        topic_name: &str,
        type_name: &str,
        key_descriptors: &KeyDescriptors,
    ) -> Result<Topic, DdsError> {
        let key = (topic_name.to_string(), type_name.to_string());
        if let Some(topic) = self.topics.get(&key) {
            if topic.key_descriptors != *key_descriptors {
                tracing::warn!(
                    topic = topic_name,
                    type_name = type_name,
                    "topic already exists with different key descriptors; using cached version"
                );
            }
            return Ok(topic.clone());
        }

        let c_topic_name =
            CString::new(topic_name).map_err(|_| DdsError::InvalidName(topic_name.into()))?;

        let handle = unsafe {
            let mut sertype_ptr = create_opaque_sertype(type_name, key_descriptors);
            dds_create_topic_sertype(
                self.handle,
                c_topic_name.as_ptr(),
                &mut sertype_ptr,
                ptr::null(),
                ptr::null(),
                ptr::null(),
            )
        };
        if handle < 0 {
            return Err(DdsError::CreateTopic(handle));
        }

        let topic = Topic {
            handle,
            topic_name: topic_name.to_string(),
            type_name: type_name.to_string(),
            key_descriptors: key_descriptors.clone(),
        };
        self.topics.insert(key, topic.clone());
        Ok(topic)
    }
}

impl Drop for DdsParticipant {
    fn drop(&mut self) {
        unsafe {
            dds_delete(self.handle);
        }
    }
}
