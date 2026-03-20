/// Generated bindings for Cyclone DDS C API.
pub mod bindings {
    #![allow(non_upper_case_globals)]
    #![allow(non_camel_case_types)]
    #![allow(non_snake_case)]
    #![allow(dead_code)]
    #![allow(clippy::all)]

    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

pub mod error;
pub mod participant;
pub mod qos_handle;
pub mod reader;
pub mod sertype;
pub mod writer;

pub use error::DdsError;
pub use participant::{DdsParticipant, Topic};
pub use qos_handle::{make_qos, QosHandle};
pub use reader::{DdsReader, InstanceState, ReaderRegistry, ReceivedSample};
pub use sertype::{SampleWrapper, DDS_DOMAIN_DEFAULT};
pub use writer::{DdsWriter, WriterRegistry};
