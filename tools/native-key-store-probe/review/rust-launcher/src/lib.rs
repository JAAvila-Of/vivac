#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_creation_binding.rs"]
pub mod linux_creation_binding;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_creation_record.rs"]
pub mod linux_creation_record;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_gate.rs"]
pub mod linux_gate;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_loader.rs"]
pub mod linux_loader;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_reservation.rs"]
pub mod linux_reservation;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_session.rs"]
pub mod linux_session;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_spawn.rs"]
pub mod linux_spawn;

#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_admission_storage.rs"]
pub mod linux_admission_storage;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_creation_storage.rs"]
pub mod linux_creation_storage;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_current_admission.rs"]
pub mod linux_current_admission;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_custody_anchor.rs"]
mod linux_custody_anchor;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_custody_provision.rs"]
pub mod linux_custody_provision;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_custody_root.rs"]
pub mod linux_custody_root;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_endpoint_codec.rs"]
pub mod linux_endpoint_codec;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_ipc.rs"]
pub mod linux_ipc;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_openssh_package.rs"]
pub mod linux_openssh_package;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_resident_reader.rs"]
pub mod linux_resident_reader;

#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_attended_terminal.rs"]
mod attended_terminal;
#[cfg(target_os = "linux")]
#[path = "../../../../../src/device_keys/linux_ubuntu_provider.rs"]
pub mod ubuntu_provider;

#[cfg(target_os = "linux")]
pub fn probe_unlock_admitted_openssh(
    preparation: linux_current_admission::AdmittedLaunchPreparation,
    home: &std::path::Path,
    secret: &[u8],
) -> std::io::Result<(
    linux_session::NativeSession,
    ubuntu_provider::UbuntuProvider,
)> {
    preparation.probe_unlock_admitted_openssh(home, secret)
}

#[cfg(target_os = "linux")]
pub fn probe_take_ready_proof(
    gate: &linux_gate::Gate,
    id: u64,
    epoch: u64,
) -> Result<Option<linux_gate::Proof>, linux_gate::Error> {
    gate.take_ready_proof(id, epoch)
}

#[cfg(target_os = "linux")]
pub fn probe_admitted_openssh_roles(
    preparation: &mut linux_current_admission::AdmittedLaunchPreparation,
    home: &std::path::Path,
) -> std::io::Result<()> {
    preparation.probe_openssh_roles(home)
}

#[cfg(target_os = "linux")]
pub fn probe_with_admitted_openssh_registration<T>(
    preparation: &mut linux_current_admission::AdmittedLaunchPreparation,
    home: &std::path::Path,
    operation: impl FnOnce(
        &linux_current_admission::AdmittedChildRegistration<'_>,
        &std::os::fd::OwnedFd,
    ) -> std::io::Result<T>,
) -> std::io::Result<T> {
    preparation.probe_with_openssh_registration(home, operation)
}

#[cfg(target_os = "linux")]
pub fn probe_send_keeper_terminal(
    socket: &std::os::fd::OwnedFd,
    master: &std::os::fd::OwnedFd,
    deadline: std::time::Instant,
) -> std::io::Result<()> {
    linux_ipc::registration::probe_send_keeper_terminal(socket, master, deadline)
}

#[cfg(target_os = "linux")]
pub fn probe_receive_keeper_terminal(
    socket: &std::os::fd::OwnedFd,
    owner: &linux_spawn::SpawnOwner,
    token: u64,
    deadline: std::time::Instant,
) -> std::io::Result<std::os::fd::OwnedFd> {
    linux_ipc::registration::probe_receive_keeper_terminal(socket, owner, token, deadline)
}

#[cfg(target_os = "linux")]
pub fn probe_collect_keeper_provider_sigkill(
    adopted: &mut linux_ipc::registration::KeeperAdoptedOffer,
) -> std::io::Result<()> {
    adopted.probe_collect_provider_sigkill()
}

#[cfg(target_os = "linux")]
pub fn probe_advance_pre_unlock_generation(
    adopted: &mut linux_ipc::registration::KeeperAdoptedOffer,
) -> std::io::Result<linux_ipc::registration::KeeperPreUnlockSuccessor<'_>> {
    adopted.probe_advance_pre_unlock_generation()
}

#[cfg(target_os = "linux")]
pub fn probe_launch_restricted_successor(
    successor: linux_ipc::registration::KeeperPreUnlockSuccessor<'_>,
    home: &std::path::Path,
    observe: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    successor.probe_launch_restricted_successor(home, observe)
}

#[cfg(target_os = "linux")]
pub fn probe_unlock_attended_successor(
    successor: linux_ipc::registration::KeeperPreUnlockSuccessor<'_>,
    home: &std::path::Path,
) -> std::io::Result<(
    linux_session::NativeSession,
    ubuntu_provider::UbuntuProvider,
)> {
    successor.probe_unlock_attended_successor(home)
}

#[cfg(target_os = "linux")]
pub fn probe_unlock_registered_active(
    preparation: linux_current_admission::AdmittedLaunchPreparation,
    home: &std::path::Path,
    register: impl FnOnce(
        &linux_current_admission::AdmittedChildRegistration<'_>,
    ) -> std::io::Result<linux_ipc::registration::KeeperActiveClosureSender>,
) -> std::io::Result<(
    linux_session::NativeSession,
    ubuntu_provider::UbuntuProvider,
    linux_ipc::registration::KeeperActiveClosureSender,
)> {
    preparation.probe_unlock_registered_active(home, register)
}

#[cfg(target_os = "linux")]
pub fn probe_launch_admitted_pair(
    preparation: &mut linux_current_admission::AdmittedLaunchPreparation,
    specs: [linux_spawn::SpawnSpec; 2],
) -> std::io::Result<linux_current_admission::AdmittedChildRegistration<'_>> {
    preparation.launch_pair(specs)
}

#[cfg(target_os = "linux")]
pub fn probe_observe_admitted_children(
    preparation: &mut linux_current_admission::AdmittedLaunchPreparation,
) -> std::io::Result<Option<Vec<linux_spawn::ExitReceipt>>> {
    preparation.observe_direct_exits()
}
