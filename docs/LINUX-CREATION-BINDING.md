# Initial native creation binding

This selects the bounded possession check used by the future
[attended creation producer](LINUX-CREATION-ADMISSION.md). The check establishes
that the owned native agent can sign with the exact prepared credential. It
does not establish attended secret generation, trusted installation selection,
process closure, persistent admission or organizational authority.

Normal Linux custody remains unavailable until the complete producer, admission
transaction and endpoint lifecycle are implemented and accredited. A codec or
signature test alone does not satisfy those requirements.

The experimental implementation is in
[`linux_creation_binding.rs`](../src/device_keys/linux_creation_binding.rs).
The disposable `rust-creation-binding` experiment in
[`run_lifecycle_probe.py`](../tools/native-key-store-probe/review/run_lifecycle_probe.py)
uses an owned OpenSSH agent and loader with synthetic credentials. It checks the
complete record binding, independently verifies the signature with native
OpenSSL, rejects an altered message, and observes native closure. Separate cases
refuse a different key or protected comment without signing. Its output keeps
producer origin, creation admission and custody accreditation false: the record
is supplied by the fixture, rather than constructed by a trusted producer.

The later [composed producer experiment](LINUX-CREATION-ADMISSION.md#authority-and-inputs)
constructs the record from its own attended preparation, performs this check and
consumes the real closed session before publication. It keeps the same false
operational-authority flags. Its private producer core is separate from the
observer protocol and selects the experimental Ubuntu provider internally. That
selection does not admit the complete backend or enable Linux custody.

## Producer context

The producer constructs the version 2 record from its retained installation,
fresh creation identity, generated device/credential identity and exact prepared
encrypted package. It must not accept a caller-supplied record, key, digest,
package path or success flag as evidence that creation occurred.

The [record codec](../src/device_keys/linux_creation_record.rs) validates the
complete canonical version 2 representation before the challenge is issued.
The challenge constructor is not an admission constructor: a structurally valid
record can still have untrusted origins. The complete producer must retain and
revalidate its own objects around native signing and publication.

## Fixed signing bytes

The message contains exactly:

1. ASCII `vivac.local.creation-binding.v1` followed by one zero byte.
2. The 32-byte SHA-256 digest of the complete canonical version 2 record.
3. A fresh 32-byte nonce from the operating system cryptographic random source.

There is no textual hexadecimal encoding, caller-controlled suffix, alternate
domain or optional field in the signing bytes. The record digest binds its
installation, creation, device and credential identifiers, public key, encrypted
key and manifest digests and lengths, and profile/policy revisions. Changing any
of those inputs changes the required message or makes the record invalid.

Linux nonce generation uses its nonblocking cryptographic random syscall and
requires all 32 bytes. Failure or a short result refuses without a fallback.
The nonce is not a passphrase and is never used as a signing seed.

The in-memory challenge retains a monotonic issue time after encoding and random
generation. Verification requires elapsed time strictly below 30 seconds; an
invalid or reversed time interval refuses. This deadline covers the native
check, not the earlier human interaction that confirms the generated secret.

## Native verification and lifetime

Only the producer's owned, previously validated native agent may receive this
message. Its exact public key and protected comment must match the prepared
credential before signing. The agent, loader, package lease and live-peer
identities remain retained under the existing native-session contract.

The result must be one bounded Ed25519 signature verified strictly against the
record's expected public key and the exact signing bytes. The endpoint cannot
select a replacement verification key. Enrollment proofs, endpoint proofs,
wrong signatures, changed context or expired challenges refuse.

Verification consumes its challenge on every attempt, including failure. A
verified binding has private fields and no public constructor, deserializer,
clone or persistent representation. Its record comparison validates the complete
canonical record again and compares its digest. It is possession evidence only;
it cannot be imported as producer evidence or converted into an admitted
credential by parsing metadata.

After successful native verification, the producer must still close admission,
drain its owned temporary native work, observe required exits and publish the
resident package and operational record in the prescribed order. Uncertain exit
or publication cannot be repaired by retaining or replaying this signature.

## Acceptance

Tests require a positive signature for a complete record and refusal after any
bound field, nonce, expected key, purpose or signature changes. They also cover
invalid records, random-source failure and exact monotonic deadline boundaries.
Public deterministic signing fixtures belong only in tests.
Compiler checks must also reject reusing the same challenge after either a
successful verification path or an attempt whose error was retained or ignored.

Native acceptance additionally requires the actual owned OpenSSH agent, exact
protected identity, package revalidation, cancellation, lock ordering and exit
observations. A fake signer or caller-supplied key can exercise the codec, but
cannot accredit that native flow or authorize publication.
