# Linux device-key custody requirements and service-profile design

The accredited Linux implementation is the
[OpenSSH profile](LINUX-OPENSSH-PROFILE.md) on Ubuntu 24.04 amd64. It provides
attended creation in the provisioned account store and private reopening and
endpoint verification. The [public custody CLI design](LINUX-CUSTODY-CLI.md)
specifies attended reopening and fresh local possession checks. Enrollment
signing requires a separate authorization contract. Public proof verification
and ordinary local work remain available.

The initial GNOME Keyring provider selection has been rejected for new signing
credentials after reviewing its legacy persistent format. The [provider
review](LINUX-DEVICE-KEYS-REVIEW.md) records the evidence and remaining gates.
The [common-mechanism comparison](LINUX-KEY-PROTECTION-OPTIONS.md) explains
the selection of the dedicated OpenSSH profile. The rest of this document
retains the acceptance contract for experimental Secret Service providers.
Its D-Bus, collection and package requirements apply to those candidates.

## Trust and protection

The target is one private, persistent provider profile per credential
store, reached over a private local D-Bus with service activation disabled.
It never adopts the desktop bus, login keyring, default profile, SSH agent or
a caller-supplied Secret Service endpoint. Within the owned profile, the
credential collection is selected explicitly and bound to its stored identity;
a default alias alone is not that binding. Only the secrets component runs.
Provider executables and supported versions must be selected and validated
before launch; an arbitrary executable found through a caller-controlled PATH
is not an accredited provider.

The person operating the Linux client holds the store passphrase. The provider holds
the unlocked credential during an explicit local session; vivac retrieves the
versioned signing payload only to validate it or sign the bounded enrollment
challenge. The synchronization coordinator receives neither the passphrase nor
the seed. An SSH target running this client is a trusted endpoint with access
to the unlocked material; SSH does not preserve secrecy from its administrator
or processes acting as that operating-system user. Local account access is
not organizational membership, human approval or authority to enroll a device.

Secret Service is an interface, not an at-rest encryption guarantee. The
profile must use a nonempty passphrase and a reviewed encrypted persistent
format. A binary file header, DH negotiation or successful retrieval
alone cannot establish adequate protection. Activation requires review of the
exact supported provider's key derivation, integrity protection, storage
format and failure behavior. Unsupported formats, changed protection and
unverifiable provenance refuse; no plaintext or generic-provider fallback is
allowed. This contract does not introduce custom encryption or claim hardware
nonexportability, complete memory erasure or protection from a compromised host.

## Explicit session lifecycle

This service-based design considered `vivac device-key unlock
<absolute-directory> --hold [--json]`. The example describes an explicit
foreground lifetime, with the candidate staying usable while the command
remains active. The public OpenSSH command contract is a separate decision.
The existing Windows and macOS commands retain their own lifecycle.

1. `create` requires a controlling terminal and a new private destination. It
   reads and confirms a passphrase with echo disabled, prepares an isolated
   provider and stores a newly generated credential. It stops its temporary
   provider and bus, verifies the final persistent revision, then publishes a
   complete package. The committed credential starts unavailable for signing.
2. `unlock --hold` validates an existing package before starting its own private
   bus and foreground provider. It reads the passphrase from the controlling
   terminal and verifies the exact credential, encrypted revision and native
   lock state before publishing session readiness. It never creates a missing
   collection or accepts a replacement store while unlocking.
3. While that session remains alive, separate local commands may use the exact
   store without a terminal. `show` and `prove` never prompt, start a provider,
   unlock, repair or regenerate the store. Without a validated live session they
   refuse. Proof output retains the existing possession-only contract.
4. `lock` closes admission of new credential operations, drains or cancels
   admitted operations, locks the owned collection and terminates the owned
   provider and bus. Only then may it acknowledge that the session ended. A
   proof already returned is not revoked. Failure to establish shutdown is
   reported as uncertain, never as a successful lock.
5. Foreground termination, terminal hangup, bus/provider exit or loss of the
   validated runtime endpoint invalidates readiness. Session cleanup targets
   only owned child processes. Reconnecting requires explicit unlock; there is
   no automatic passphrase retention, restart or desktop-service fallback.

The foreground session emits one bounded readiness receipt, then waits without
a spinner or periodic output. Its successful final exit follows verified
shutdown; startup or runtime failure returns a nonzero exit. JSON readiness
is not a terminal exit receipt or a human-authentication assertion. Runtime
state is separate from the persistent package and is not a reusable capability
merely because a socket, PID file or old receipt exists.

The implementation must enforce one live session per store, serialize lock
against signing, authenticate local peers and bind every request to the exact
store and current provider instance. It must retain bus/provider identities
across the request, refuse owner replacement, and restrict communication to
local Unix sockets under owned private directories. Ambient D-Bus addresses,
activation configuration and other desktop environment values do not select
its authority. Process IDs alone never authorize cleanup or establish identity.

Passphrases follow the existing macOS input bounds: valid UTF-8, 16 through
512 bytes, without NUL. They are not accepted through CLI arguments,
environment, caller stdin, files or harness prompts. Vivac may pass a password
it has read to its owned provider over a private pipe; it is never printed or
stored. Secret buffers are zeroized on normal scope exit, without promises
about provider internals, swap, debuggers or abnormal process termination.

All native calls and shutdown have finite, tested deadlines. A timeout is an
operation failure, never evidence that a prompt was refused or a store locked.
No operation may execute a provider Prompt: the adapter must inspect raw reply
paths before a convenience wrapper can run a prompt internally. Checking the
collection's lock state first does not eliminate a later prompt race.

## Persistent package and revision binding

The package stays outside Git working trees and `.vivac`, under an existing
private local parent, with owner-only directories and files. Validate every
component through retained handles; reject symlinks, extra hard links, foreign
ownership, permissive modes or ACLs, unsupported filesystems and unexpected
members. A package schema must enumerate every supported native member,
including any alias/lock files, before the backend is implemented. Native files
are not trusted just because they lie under the chosen profile path.

For the conditional Secret Service candidate, the protected item contains the
common 84-byte versioned credential payload. The OpenSSH candidate uses the
native protected layout specified in its profile; it never places the seed in
a comment returned as public metadata by the agent.
Selection requires one exact collection and one item with the complete expected
attribute set. Attributes contain only public opaque identifiers. The adapter
validates payload length/version, device and credential identities, public-key
binding and item identity around retrieval. A provider's `text/plain` reply
does not make the payload text; no secret is decoded or logged as prose.

A bounded public manifest binds format/protection version, device and
credential identifiers, public key, native member identities and encrypted
revision digests. Each native database is at most 1 MiB; the manifest is at most
2 KiB. The complete member schema must also fix aggregate bytes, entry counts,
IPC reply sizes and operation work before activation. These are hard bounds,
not permission to truncate inspection. The manifest has no seed or passphrase
and is not enrollment authority.

Creation publishes only a fully verified new package and never overwrites a
destination. Interrupted preparations remain distinguishable from committed
stores; cleanup removes only preparations still owned by that operation.
Subsequent reads verify the committed encrypted revision before and after
provider access so a cached key cannot hide changed disk contents. If native
unlock or lock legitimately rewrites persistent bytes, its exact supported
transition and atomic manifest update must be accredited before activation;
silently accepting a new digest is not a solution. Concurrent mutation refuses.

Provider restart must recover the same protected key from the committed package
after a fresh terminal unlock. Recovery, rotation, external password changes,
credential copying and enrollment revocation remain separate operations.

## SSH and remote work

| Situation | Required behavior |
|---|---|
| Remote machine only builds or tests | No credential store or unlock is required just to execute that work. The originating session retains its explicit recording destination. |
| Interactive SSH client needs this machine's signing key | The person opens an explicit foreground unlock session on that machine; no desktop keyring or GUI is required. |
| Agent runs without a TTY | It may prove possession using an already validated local session. It cannot create or unlock a store by inventing a password channel. |
| Unlock SSH session disconnects | Terminal hangup invalidates the foreground session and stops its owned children. After uncertain disconnect, clients must not infer readiness from old runtime files. |
| Separate signing caller disconnects | Do not assume cancellation. A returned or lost proof is not shared admission; reconcile the operation before retrying any future stateful request. |
| Work continues from another device | That device needs its own protected credential and explicit enrollment under current authority; copying this package or forwarding the private bus is unsupported. |

SSH authentication does not grant vivac permissions. The signing credential
does not define a person, lane, project or harness identity. This profile does
not implement remote delegation, tree synchronization, context delivery or
automatic attribution. Those flows must separately link their originating work
and authorized context. Ordinary local capture remains independent of this
provider, network availability and shared admission.

## Acceptance before activation

Use disposable Linux environments, synthetic credentials and the development
executable. Never target a maintainer's login keyring, SSH credentials, live
tree or installed vivac. No private package or passphrase is uploaded as a CI
artifact. Record exact OS, provider and dependency versions with results.

| Case | Required evidence |
|---|---|
| Supported encrypted profile | Reviewed provider format and cryptography; correct password succeeds after full provider/bus restart; empty/wrong passwords and plaintext format refuse. A header check alone does not pass. |
| No desktop | Create and foreground unlock through a pseudo-terminal with DISPLAY, WAYLAND_DISPLAY and ambient session bus absent; prove from a separate process without a TTY. |
| Actual SSH lifecycle | On an isolated loopback SSH fixture, disconnect the foreground unlock terminal and verify refusal, owned-child shutdown and explicit recovery after reconnect. A PTY-only test does not cover this row. |
| Isolation | A second profile, ambient keyring and malicious service on an unrelated bus remain untouched; a missing/replaced owned provider refuses without activation or fallback. |
| No prompts | Provider replies requiring Prompt at create/read/lock boundaries are rejected without invoking it, including a race after the preliminary lock check. |
| Exact binding | Wrong account, extra attributes, duplicate items, changed collection/item identities, malformed payload and mismatched public key refuse. |
| Package safety | Links, ACL/mode/owner changes, unexpected members, unsupported filesystem, oversized files and a second creator/session refuse without modifying the existing package. |
| Integrity and persistence | Changed/corrupt files refuse even with a warm provider cache; native rewrites are either exactly supported or refused; cold restart recovers the committed key. |
| Concurrency and interruption | Lock races, startup/read crashes, terminal hangup, bus/provider death and lost readiness receipts never reuse stale sessions or produce false lock success; cleanup cannot affect another process tree. |
| Proof boundary | Existing challenge bounds and exact signatures hold; altered/expired challenges refuse; no arbitrary-message signing or enrollment approval is introduced. |
| Resource bounds | The package schema and IPC adapter enforce declared limits before allocating or inspecting unbounded data; all native failure paths complete within their declared deadlines. |
| Regression | Current Windows/macOS custody behavior, Linux unsupported-mode behavior until activation, public verification and ordinary local capture retain their contracts. |

Dependency admission is independent of native fixture success. The probe's
`secret-service = 6.0.0-rc.1` is not approved for production by this document.
Before implementation selects a graph, review current maintenance/advisories,
MSRV, license and notice requirements, prompt-control APIs and reproducible
locked native builds. Do not suppress a dependency-policy failure or promote a
prerelease silently. If the provider or adapter cannot satisfy this complete
contract, keep Linux signing unavailable and revisit the design.

## Primary references

- [Secret Service specification: transfer encryption, prompts and excluded guarantees](https://specifications.freedesktop.org/secret-service/latest-single/)
- [GNOME daemon startup and lifecycle](https://wiki.gnome.org/Projects/GnomeKeyring/RunningDaemon)
- [GNOME daemon implementation](https://github.com/GNOME/gnome-keyring/blob/main/daemon/gkd-main.c)
- [GNOME persistent binary format implementation](https://github.com/GNOME/gnome-keyring/blob/main/pkcs11/secret-store/gkm-secret-binary.c)
- [Implemented device-key behavior](DEVICE-KEYS.md)
- [Possession proof boundary](DEVICE-ENROLLMENT-PROOFS.md)
- [Shared authority and local capture](TEAM-AUTHORITY.md)
