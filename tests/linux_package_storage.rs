#![cfg(target_os = "linux")]

#[allow(dead_code)]
#[path = "../src/device_keys/linux_attended_terminal.rs"]
mod attended_terminal;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_creation_ciphertext.rs"]
mod creation_ciphertext;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_attended_creation_input.rs"]
mod creation_input;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_admission_storage.rs"]
mod linux_admission_storage;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_creation_binding.rs"]
mod linux_creation_binding;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_creation_producer.rs"]
mod linux_creation_producer;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_creation_record.rs"]
mod linux_creation_record;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_creation_storage.rs"]
mod linux_creation_storage;
// Preserve the public re-exports used by the standalone native helpers.
#[allow(dead_code)]
#[path = "../src/device_keys/linux_current_admission.rs"]
pub mod linux_current_admission;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_custody_anchor.rs"]
mod linux_custody_anchor;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_custody_root.rs"]
mod linux_custody_root;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_endpoint_codec.rs"]
mod linux_endpoint_codec;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_gate.rs"]
mod linux_gate;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_ipc.rs"]
mod linux_ipc;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_loader.rs"]
mod linux_loader;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_openssh_package.rs"]
mod linux_openssh_package;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_reservation.rs"]
mod linux_reservation;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_resident_reader.rs"]
mod linux_resident_reader;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_session.rs"]
pub mod linux_session;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_spawn.rs"]
pub mod linux_spawn;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_opening_publication.rs"]
mod opening_publication;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_package_storage.rs"]
mod package;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_ubuntu_provider.rs"]
mod ubuntu_provider;
