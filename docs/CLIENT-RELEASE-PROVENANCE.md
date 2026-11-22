# Client release provenance

A client that holds team content keys needs a distribution path controlled by
the organization, independently of the synchronization service. Signed release
provenance can authenticate an official archive's publisher and source commit.
It does not establish that the organization approved that commit, that its code
is safe, or that an administrator has authority to enroll a team.

This contract supports the trusted-client prerequisite in
[team bootstrap](TEAM-BOOTSTRAP.md). It does not replace the separate
organization and principal acceptance required there.

## Trust inputs

The organization supplies the verifier, trust roots and approved full source
commit through a channel independent of the candidate archive and the sync
operator. A candidate may supply its archive and signature bundle; it cannot
choose the expected identity, commit or roots. A root file delivered beside an
archive is not trusted merely because the two agree.

For official GitHub releases, the expected identity is:

| Field | Required value |
|---|---|
| Repository | `JAAvila-Of/vivac` |
| Repository identifier | `1352788684` |
| Repository owner identifier | `69764058` |
| Certificate issuer | `https://token.actions.githubusercontent.com` |
| Signing workflow | `https://github.com/JAAvila-Of/vivac/.github/workflows/release.yml@refs/heads/main` |
| Source ref | `refs/heads/main` |
| Runner environment | `github-hosted` |
| Predicate type | `https://slsa.dev/provenance/v1` |

The source ref is the workflow's source ref, not the release tag. The full source
commit must be approved independently; resolving `latest`, reading a tag or
copying a commit out of the candidate bundle does not provide that approval.
The numeric identifiers prevent an offline name-only policy from accepting a
different repository or owner that later reuses the same name.

GitHub CLI versions before 2.102.0 are outside this verification profile.
That release fixes workflow path-prefix matching; earlier versions also had a
regular-expression escaping issue. The verifier binary itself remains a trust
input: printing an acceptable version does not authenticate it.

## Offline verification

Obtain the signed bundle and an independently provisioned `trusted_root.jsonl`.
GitHub CLI can acquire roots using
`GH_TELEMETRY=false DO_NOT_TRACK=true gh attestation trusted-root`; this is an
explicit network operation, performed before entering the offline environment.
Retain the approved root file and its digest. Do not let the archive choose a
custom TUF endpoint or replace that file.

Inside an isolated environment, invoke the trusted verifier with the candidate
archive as data. For a Linux x86_64 release archive:

```sh
GH_TELEMETRY=false DO_NOT_TRACK=true \
gh attestation verify ./vivac-x86_64-unknown-linux-musl.tar.gz \
  --bundle ./bundles.jsonl \
  --custom-trusted-root ./trusted_root.jsonl \
  --repo JAAvila-Of/vivac \
  --signer-workflow JAAvila-Of/vivac/.github/workflows/release.yml@refs/heads/main \
  --cert-oidc-issuer https://token.actions.githubusercontent.com \
  --source-digest "$APPROVED_SOURCE_COMMIT" \
  --source-ref refs/heads/main \
  --deny-self-hosted-runners \
  --digest-alg sha256 --format json --hostname github.com
```

Use a private, immutable input snapshot. Refuse verification failures. Only after
a successful process exit may the JSON output be used for additional policy
checks. In `verificationResult.signature.certificate`, compare the numeric
identifiers and all identity fields above exactly, including
`sourceRepositoryDigest`, `sourceRepositoryURI` and `subjectAlternativeName`.
These are certified identity fields; a workflow-controlled provenance predicate
is not a substitute for them.

Within that same verified result, require a `statement.subject` entry whose
`name` equals the expected target archive and whose `digest.sha256` equals the
SHA-256 of the retained input bytes. GitHub CLI verifies the artifact digest;
it does not enforce the target filename. Require the expected predicate type
and verified signing timestamps. Never accept an arbitrary imported JSON result
as proof that verification ran.

Verification does not require extracting or running the candidate executable.
Keep any later installation tied to the exact verified bytes; replacing the
archive after verification invalidates the result. This manual contract does
not add signature enforcement to `vivac update`, whose archive path checks
`SHA256SUMS`. That checksum provides integrity comparison, not an independent
publisher identity when fetched from the same channel as the archive.

An organization may instead build from reviewed source using its own controlled
toolchain and distribution system. Self-hosting does not require a GitHub user
account or a vendor control plane; official release provenance is one
distribution option, not a team admission protocol.

## Automated offline probe

[`tools/client-provenance-probe/verify.py`](../tools/client-provenance-probe/verify.py)
automates the checks above using Python's standard library and a separately
provisioned GitHub CLI. It runs on Linux; its process and file isolation have
been exercised on Ubuntu 24.04. It explicitly refuses other host platforms.
The archive target may be any of the five official release targets; selecting a
Windows or macOS archive does not run that executable or validate that host OS.

For example, inside a private offline Linux environment:

```sh
python3 -B tools/client-provenance-probe/verify.py \
  --archive ./vivac-x86_64-unknown-linux-musl.tar.gz \
  --bundle ./bundles.jsonl \
  --gh /trusted/gh --gh-sha256 "$APPROVED_GH_SHA256" \
  --roots /trusted/trusted_root.jsonl \
  --roots-sha256 "$APPROVED_ROOTS_SHA256" \
  --source-commit "$APPROVED_SOURCE_COMMIT" \
  --target x86_64-unknown-linux-musl
```

The hashes and commit in this example are independent organizational inputs,
not values copied from the candidate or sync server. A verifier hash binds the
executable to that expectation; neither the hash nor the version string
establishes approval by itself. The probe admits stable GitHub CLI 2.x versions
from 2.102.0 onward; other major versions require a profile review.

The probe copies bounded regular inputs into a private temporary directory,
refusing symbolic links, hard links and special files. It verifies the root and
verifier hashes before executing only the private verifier copy. The verifier
receives a fresh home/configuration and a fixed environment without inherited
tokens or proxies. Version inspection and verification both receive fixed
`GH_TELEMETRY=false` and `DO_NOT_TRACK=true`, overriding inherited settings.
GitHub CLI telemetry can
otherwise spawn a detached sender that outlives its original process group;
disabling the sender is required even in a network-isolated environment.
Input snapshots are checked again after execution. Version
inspection has a five-second deadline and signature verification has thirty
seconds; stdout is limited to 2 MiB and stderr to 64 KiB. Archive and verifier
inputs are limited to 128 MiB each, the bundle to 4 MiB and roots to 1 MiB.
Candidate executable bytes are never extracted or run.

Exit zero writes one deterministic JSON description of the verified archive
name, SHA-256, source commit, publisher identity and verifier/root hashes.
Failure writes no success JSON. The JSON is descriptive output, not a signed
authorization or an importable trust token. Temporary snapshots are removed
before exit; a later installation must preserve or reverify the exact bytes.
This probe does not install a client, authorize a team or change `vivac update`.
Python and GitHub CLI are probe tooling, not new Vivac runtime dependencies.

The automated suite is retained in
[`test_verify.py`](../tools/client-provenance-probe/test_verify.py). Run it only
inside an isolated Linux environment, with public fixture assets mounted read
only, an empty private work directory owned by the test UID with mode `0700`,
and a separate output directory:

```sh
python3 -B tools/client-provenance-probe/test_verify.py \
  --assets-dir /assets --work-dir /data --output-dir /out
```

The suite uses the published v0.22.1 Linux x86_64 archive and its bundle, GitHub
CLI 2.102.0 and a retained approved root snapshot. It fixes their digests in the
test source; it never downloads replacements. It separates real signature
verification from deliberately trusted fake-verifier tests of process failures,
JSON policy and input guards. Fake-verifier success does not establish signed
provenance. Give the test environment one CPU, 512 MiB memory with no additional
swap, 64 PIDs, no network, a read-only root, no capabilities or credentials, and
an owned disposable filesystem for its private work. The suite includes FIFOs,
links, an oversized sparse file and bounded subprocesses in that filesystem.

[`test_targets.py`](../tools/client-provenance-probe/test_targets.py) uses the
same verifier, bundle and roots with all five retained v0.22.1 release archives.
With those archives in the read-only assets directory, it checks each matching
target, every mismatched archive/target pair, and one altered copy of each
archive. It fixes the archive digests in the test source. Run it with the same
isolation and fresh-directory requirements:

```sh
python3 -B tools/client-provenance-probe/test_targets.py \
  --assets-dir /assets --work-dir /data --output-dir /out
```

These are signature and target-binding tests over release bytes. They provide
no evidence that a Windows or macOS executable ran, that a native key store
works, or that team enrollment is authorized on those systems.

## References

- [GitHub CLI verification contract](https://cli.github.com/manual/gh_attestation_verify)
- [Offline trust-root acquisition](https://cli.github.com/manual/gh_attestation_trusted-root)
- [Workflow-prefix matching correction](https://github.com/cli/cli/security/advisories/GHSA-wjmr-j3rp-mh2g)
- [Earlier identity escaping correction](https://github.com/advisories/GHSA-mm27-mwq9-fr5g)
- [GitHub CLI telemetry environment controls](https://cli.github.com/manual/gh_help_environment)
