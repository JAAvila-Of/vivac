# Candidate installation-root reader fixture

This experiment tests the shared Linux-test-only binding codec and root-reader
mechanics against the [installation-root contract](../../../docs/LINUX-CUSTODY-ROOT.md).
It does not provision or authenticate the origin of the operational system
prefix, create a credential, admit a key or enable Linux custody.

The runner prepares public synthetic filesystem trees in a new disposable
ext-family store using a bounded archive copied into the stopped container.
Administrative directories and binding files have actual UID/GID 0 ownership;
private state belongs to UID/GID 65534. The observer independently verifies those
actual owners before invoking the helper, including each intentionally invalid
ownership case. The runtime keeps UID/GID 65534, no
capabilities, NNP, no network, CPU1, 512 MiB RAM with no additional swap,
128 PIDs and bounded tmpfs. It cannot repair administrative objects to make a
test pass. No ownership-policy parameter makes the current user an administrator.

For this mixed-owner tar stream, the runner uses `docker cp` without `--archive`.
In the selected [Docker 28.2.2 daemon](https://github.com/moby/moby/blob/v28.2.2/daemon/archive_tarcopyoptions_unix.go),
the archive option selects the container's configured user as a chown override;
the [default extraction options](https://github.com/moby/moby/blob/v28.2.2/daemon/archive_tarcopyoptions.go)
retain the archive's mapped identities. Observed owners, rather than tar headers
or the flag's name, determine whether the prepared fixture is accepted.

The Python harness selects only synthetic fixture paths and passes an open
descriptor to the Rust helper. This is not the future operational client's
root-selection API. The reader follows the fixed relative hierarchy from that
retained synthetic filesystem root and applies the selected administrative and
private-state policies. Structural validity cannot establish who provisioned it.

| Case | Required observation |
|---|---|
| `valid` | Canonical binding and the complete fixed hierarchy open and revalidate |
| `wrong-root-owner` | A synthetic filesystem root owned by the runtime user refuses |
| `writable-ancestor` | A group/other-writable administrative ancestor refuses |
| `wrong-prefix-mode` | A traversable custody prefix with the wrong mode refuses |
| `wrong-slot-owner` | A user-owned administrative account slot refuses |
| `binding-owner` | A user-owned binding refuses |
| `binding-mode` | A binding with mode 0664 refuses |
| `binding-malformed` | A noncanonical binding with a trailing newline refuses |
| `binding-oversized` | A 513-byte binding refuses before unbounded reading |
| `binding-identity` | A binding for a different numeric UID refuses |
| `ancestor-symlink` | An administrative ancestor symlink refuses |
| `binding-symlink` | A binding symlink refuses |
| `binding-hardlink` | A binding with multiple hard links refuses |
| `binding-fifo` | A FIFO binding refuses without blocking the reader |
| `missing-binding` | Missing installation binding refuses without creating one |
| `state-mode` | Private state with mode 0755 refuses |
| `state-acl` | A real access ACL on private state refuses while mode remains 0700 |
| `snapshot-private-replacement` | Replacing the retained packages directory invalidates the attempt |
| `unrelated-entry` | Adding and removing an unrelated admission entry leaves the unchanged root snapshot valid |

An independent observer verifies the live host's executable, credentials,
lineage, pidfd and stable process identity before releasing observation barriers.
The Rust host runs four pressure threads. Final success requires observed host
exit code 0 and resource release after that observation. There are no native
custody processes or publisher children in this experiment.

The observer compares bounded fixture names, object metadata, regular-file
bytes and symlink targets before and after the test without following links or
reading FIFOs. A final equality check is fixture-integrity evidence, not a
syscall trace, a power-loss test or proof that the reader issued no transient
writes. The source review must separately establish the non-writing reader path.

All producer-origin, provisioning, trusted-root bootstrap, admission, custody,
endpoint-authority, recovery, MCP/server and lock-acknowledgement fields remain
false. The report's `candidate_root_reader_observed` field credits only the
complete synthetic-case matrix. Helper and observer digests identify the exact
tested artifacts.

## Remaining gates

This experiment cannot accredit the origin of the real administrative prefix.
It also does not implement or accredit administrator provisioning durability,
binding replacement by an administrator during a live attempt, actual bind-mount
rejection, resident package/admission composition, producer evidence or the
complete creation-admission transaction. Their native acceptance remains
required before operational use. Kernel mount/symlink resolution flags in the
candidate source do not substitute for those missing cases.

## Reproduction

Build the auxiliary crate in a clean separate target, then invoke the runner
through the shared RustWork coordinator with `--family rust` and a fresh absolute
report directory:

```text
python run_lifecycle_probe.py --experiment rust-root-reader --image LOCAL_UBUNTU_IMAGE --build-volume ORIGINAL_HELPER_VOLUME --launcher-build-volume NEW_RUST_LAUNCHER_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```
