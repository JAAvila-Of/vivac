# Isolated coordinator authority probe

This crate exercises the candidate signed authority decoder and Linux persistence
boundary. Its trust roots and signing seeds are published test fixtures. It
exposes no network listener, account commands or production bootstrap interface.
Running the binary prints that limitation and exits with status 2.

The durable module is included only here, outside the product's runtime and
artifact SQLite process. The product's manifest and dependency policy remain
separate. See the [storage contract](../../docs/TEAM-AUTHORITY-STORAGE.md) for the
trust model, checkpoint limitations, transaction requirements and Linux VFS
boundary.

The pending-enrollment experiment reuses the product's proof parser and strict
verifier. Its local error adapter supplies diagnostics only. Pending challenges
and possession observations do not enroll a device, consume a nonce or authorize
an account. Their cache is outside the authority checkpoint; see the storage
contract before interpreting a historical observation.

The [approval requirements](../../docs/DEVICE-ENROLLMENT-ASSERTIONS.md) describe
the separate verification boundary still needed before device admission. The
experimental Passki/Graviola assertion profile uses public signing fixtures;
it exposes no approval command and does not establish human interaction. Its
declared Linux CPU profile, strict input bounds and in-memory ceremony state
are part of the experiment, not a production enrollment service.

Run the Linux tests with a private temporary directory on the supported local
filesystem. Compilation and native tests must use the workspace Rust coordinator
where required. Keep source snapshots fixed for each verification run and retain
failed runs as well as passing ones. Resource limits and network isolation belong
to the runner; this crate does not configure the host or install a service.
