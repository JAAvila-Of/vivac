#!/usr/bin/env python3
"""Disposable OpenSSH interoperability observations, never custody approval."""

import argparse
import base64
import binascii
import copy
import errno
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import time


TIMEOUT = 15
OUTPUT_LIMIT = 32 * 1024
KEY_LIMIT = 64 * 1024
FRAME_LIMIT = 16 * 1024
PASSWORD = b"vivac-openssh-public-fixture-only-2026"
WRONG_PASSWORD = b"different-public-fixture-only-2026"
ALPHABET = "0123456789abcdefghjkmnpqrstvwxyz"
DOMAIN = b"vivac.device.enrollment-proof.v1\0"
PROMPT = re.compile(
    rb"(?i)((?:enter (?:same )?passphrase[^\r\n]*:|bad passphrase[^\r\n]*:))\s*\Z"
)


class ProbeError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ProbeError(message)


def stop(process):
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=2)


def child_setup():
    import resource

    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


def environment(root, agent_socket=None):
    result = {
        "PATH": os.defpath,
        "HOME": str(root / "home"),
        "LANG": "C",
        "LC_ALL": "C",
        "SSH_ASKPASS_REQUIRE": "never",
    }
    if agent_socket is not None:
        result["SSH_AUTH_SOCK"] = str(agent_socket)
    return result


def bounded_run(arguments, env, fixture_input=None):
    require(fixture_input is None or len(fixture_input) <= 2048, "fixture input exceeded its bound")
    process = subprocess.Popen(
        arguments, env=env,
        stdin=subprocess.DEVNULL if fixture_input is None else subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT, start_new_session=True, preexec_fn=child_setup,
        bufsize=0,
    )
    output = bytearray()
    deadline = time.monotonic() + TIMEOUT
    try:
        if fixture_input is not None:
            # Synthetic fixture only, through this child's private pipe. This
            # does not accredit terminal input or expose a product stdin API.
            try:
                require(process.stdin.write(fixture_input) == len(fixture_input), "fixture pipe write was incomplete")
                process.stdin.close()
            except BrokenPipeError:
                process.stdin.close()
        with selectors.DefaultSelector() as selector:
            os.set_blocking(process.stdout.fileno(), False)
            selector.register(process.stdout, selectors.EVENT_READ)
            while selector.get_map():
                require(time.monotonic() < deadline, "native operation timed out")
                for key, _ in selector.select(0.05):
                    chunk = os.read(key.fd, 4096)
                    if not chunk:
                        selector.unregister(key.fileobj)
                        continue
                    output.extend(chunk)
                    require(len(output) <= OUTPUT_LIMIT, "native output exceeded its bound")
            remaining = deadline - time.monotonic()
            require(remaining > 0, "native operation timed out")
            return process.wait(timeout=remaining), bytes(output)
    except subprocess.TimeoutExpired as error:
        raise ProbeError("native operation timed out") from error
    finally:
        stop(process)
        process.stdout.close()


def pty_run(arguments, env, responses, *, require_responses=True, allow_retry_eof=False,
            inherited_fds=()):
    import fcntl
    import pty
    import termios

    master, slave = pty.openpty()

    def setup():
        child_setup()
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

    process = None
    output = bytearray()
    sent = 0
    prompt_end = 0
    eof_sent = False
    deadline = time.monotonic() + TIMEOUT
    try:
        process = subprocess.Popen(
            arguments, env=env, stdin=slave, stdout=slave, stderr=slave,
            preexec_fn=setup, pass_fds=inherited_fds,
        )
        os.close(slave)
        slave = -1
        os.set_blocking(master, False)
        with selectors.DefaultSelector() as selector:
            selector.register(master, selectors.EVENT_READ)
            while selector.get_map():
                require(time.monotonic() < deadline, "PTY operation timed out")
                for key, _ in selector.select(0.05):
                    try:
                        chunk = os.read(key.fd, 4096)
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
                        chunk = b""
                    if not chunk:
                        selector.unregister(master)
                        continue
                    output.extend(chunk)
                    require(len(output) <= OUTPUT_LIMIT, "PTY output exceeded its bound")
                    match = PROMPT.search(output)
                    if match is None or match.end(1) <= prompt_end:
                        continue
                    prompt_end = match.end(1)
                    require(
                        not termios.tcgetattr(master)[3] & termios.ECHO,
                        "native password prompt left terminal echo enabled",
                    )
                    if sent < len(responses):
                        os.write(master, responses[sent] + b"\n")
                        sent += 1
                    elif allow_retry_eof and not eof_sent:
                        os.write(master, b"\x04")
                        eof_sent = True
                    else:
                        raise ProbeError("unexpected native password prompt")
            remaining = deadline - time.monotonic()
            require(remaining > 0, "PTY operation timed out")
            code = process.wait(timeout=remaining)
        if require_responses:
            require(sent == len(responses), "native password prompt was not observed")
        return code, bytes(output)
    except subprocess.TimeoutExpired as error:
        raise ProbeError("PTY operation timed out") from error
    finally:
        stop(process)
        if slave >= 0:
            os.close(slave)
        os.close(master)


class Reader:
    def __init__(self, data):
        self.data = data
        self.position = 0

    def take(self, length):
        require(0 <= length <= len(self.data) - self.position, "truncated native encoding")
        result = self.data[self.position:self.position + length]
        self.position += length
        return result

    def number(self):
        return struct.unpack(">I", self.take(4))[0]

    def string(self, limit=KEY_LIMIT):
        length = self.number()
        require(length <= limit, "native string exceeded its bound")
        return self.take(length)

    def finish(self):
        require(self.position == len(self.data), "unexpected native trailing data")


def wire_string(value):
    return struct.pack(">I", len(value)) + value


def read_private_file(path):
    metadata = path.lstat()
    require(
        stat.S_ISREG(metadata.st_mode) and metadata.st_nlink == 1
        and metadata.st_uid == os.geteuid() and not metadata.st_mode & 0o077
        and 0 < metadata.st_size <= KEY_LIMIT,
        "private key file metadata was refused",
    )
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor, "rb") as source:
        actual = os.fstat(source.fileno())
        require(
            (actual.st_dev, actual.st_ino) == (metadata.st_dev, metadata.st_ino),
            "private key file changed during opening",
        )
        contents = source.read(KEY_LIMIT + 1)
    require(len(contents) <= KEY_LIMIT, "private key file exceeded its bound")
    return contents


def parse_key(path):
    return parse_key_bytes(read_private_file(path))


def parse_key_bytes(contents):
    require(0 < len(contents) <= KEY_LIMIT, "native key input exceeded its bound")
    lines = contents.splitlines()
    require(
        len(lines) >= 3 and lines[0] == b"-----BEGIN OPENSSH PRIVATE KEY-----"
        and lines[-1] == b"-----END OPENSSH PRIVATE KEY-----",
        "native key envelope was refused",
    )
    try:
        decoded = base64.b64decode(b"".join(lines[1:-1]), validate=True)
    except (ValueError, binascii.Error) as error:
        raise ProbeError("native key encoding was refused") from error
    reader = Reader(decoded)
    require(reader.take(15) == b"openssh-key-v1\0", "native key magic was refused")
    cipher = reader.string(64)
    kdf = reader.string(32)
    options = reader.string(128)
    require(reader.number() == 1, "native key count was refused")
    public_blob = reader.string(128)
    public = Reader(public_blob)
    require(public.string(32) == b"ssh-ed25519", "native public key algorithm was refused")
    public_key = public.string(32)
    require(len(public_key) == 32, "native public key length was refused")
    public.finish()
    encrypted = reader.string(KEY_LIMIT)
    require(bool(encrypted), "native key payload is empty")
    rounds = None
    if cipher == b"aes256-gcm@openssh.com":
        require(kdf == b"bcrypt", "native encrypted key KDF was refused")
        parameters = Reader(options)
        require(len(parameters.string(32)) == 16, "native bcrypt salt was refused")
        rounds = parameters.number()
        require(1 <= rounds <= 256, "native bcrypt rounds were refused")
        parameters.finish()
        require(len(encrypted) % 16 == 0, "native encrypted payload alignment was refused")
        reader.take(16)
    elif cipher == b"none":
        require(kdf == b"none" and options == b"", "native plaintext KDF was refused")
    else:
        raise ProbeError("native key cipher was refused")
    reader.finish()
    return {
        "cipher": cipher.decode("ascii"), "kdf": kdf.decode("ascii"),
        "rounds": rounds, "public_blob": public_blob,
        "public_key": public_key, "decoded": decoded,
    }


def private_write(path, data):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as destination:
        destination.write(data)
        destination.flush()
        os.fsync(destination.fileno())


def corrupt_tag(source, destination):
    parsed = parse_key(source)
    require(parsed["cipher"] == "aes256-gcm@openssh.com", "corruption source is not encrypted")
    decoded = bytearray(parsed["decoded"])
    decoded[-1] ^= 1
    encoded = base64.b64encode(decoded)
    body = b"\n".join(encoded[index:index + 70] for index in range(0, len(encoded), 70))
    private_write(
        destination,
        b"-----BEGIN OPENSSH PRIVATE KEY-----\n" + body
        + b"\n-----END OPENSSH PRIVATE KEY-----\n",
    )


def receive_exact(connection, size, deadline):
    result = bytearray()
    while len(result) < size:
        remaining = deadline - time.monotonic()
        require(remaining > 0, "agent operation timed out")
        connection.settimeout(remaining)
        chunk = connection.recv(size - len(result))
        require(bool(chunk), "agent closed a partial frame")
        result.extend(chunk)
    return bytes(result)


def agent_request(path, request):
    require(len(request) <= FRAME_LIMIT, "agent request exceeded its bound")
    deadline = time.monotonic() + TIMEOUT
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(TIMEOUT)
        try:
            connection.connect(str(path))
            remaining = deadline - time.monotonic()
            require(remaining > 0, "agent operation timed out")
            connection.settimeout(remaining)
            connection.sendall(struct.pack(">I", len(request)) + request)
            length = struct.unpack(">I", receive_exact(connection, 4, deadline))[0]
            require(0 < length <= FRAME_LIMIT, "agent reply exceeded its bound")
            return receive_exact(connection, length, deadline)
        except socket.timeout as error:
            raise ProbeError("agent operation timed out") from error


def identities(path):
    reply = Reader(agent_request(path, b"\x0b"))
    require(reply.take(1) == b"\x0c", "agent identities reply was refused")
    count = reply.number()
    require(count <= 1, "agent has unexpected additional identities")
    result = [(reply.string(128), reply.string(256)) for _ in range(count)]
    reply.finish()
    return result


def clear_agent(path):
    require(agent_request(path, b"\x13") == b"\x06", "agent identity removal failed")
    require(identities(path) == [], "agent identities were not cleared")


def sign(path, public_blob, message):
    request = b"\x0d" + wire_string(public_blob) + wire_string(message) + struct.pack(">I", 0)
    reply = Reader(agent_request(path, request))
    require(reply.take(1) == b"\x0e", "agent signature reply was refused")
    signature_blob = Reader(reply.string(128))
    reply.finish()
    require(signature_blob.string(32) == b"ssh-ed25519", "agent signature algorithm was refused")
    signature = signature_blob.string(64)
    require(len(signature) == 64, "agent signature length was refused")
    signature_blob.finish()
    return signature


def identifier():
    value = int.from_bytes(os.urandom(16), "big")
    characters = []
    for _ in range(26):
        characters.append(ALPHABET[value & 31])
        value >>= 5
    return "".join(reversed(characters))


def challenge(public_key, device_id, credential_id, *, expired=False):
    now = int(time.time())
    issued = now - 300 if expired else now
    return {
        "version": 1, "organization_id": identifier(), "principal_id": identifier(),
        "device_id": device_id, "credential_id": credential_id,
        "public_key": public_key.hex(), "operation_id": identifier(),
        "base_revision": 1, "nonce": os.urandom(32).hex(),
        "issued_at": issued, "expires_at": issued + 240,
    }


def canonical(value):
    return (
        DOMAIN + value["organization_id"].encode("ascii")
        + value["principal_id"].encode("ascii") + value["device_id"].encode("ascii")
        + value["credential_id"].encode("ascii") + bytes.fromhex(value["public_key"])
        + value["operation_id"].encode("ascii") + value["base_revision"].to_bytes(8, "big")
        + bytes.fromhex(value["nonce"]) + value["issued_at"].to_bytes(8, "big")
        + value["expires_at"].to_bytes(8, "big")
    )


def proof(value, signature):
    return {
        "challenge": value, "signature": signature.hex(), "status": "possession_only",
        "authority": "not_checked", "challenge_use": "not_checked",
    }


def verify(helper, root, env, document, name, expected_success):
    path = root / (name + ".json")
    private_write(path, json.dumps(document, separators=(",", ":")).encode("utf-8"))
    code, _ = bounded_run([str(helper), str(path)], env)
    require((code == 0) == expected_success, "proof verifier returned an unexpected result")


def drain_agent_output(agent, captured):
    try:
        while True:
            chunk = os.read(agent.stdout.fileno(), 4096)
            if not chunk:
                break
            captured.extend(chunk)
            require(len(captured) <= OUTPUT_LIMIT, "agent output exceeded its bound")
    except BlockingIOError:
        pass


def keygen_arguments(binary, path, comment, rounds):
    return [
        binary, "-q", "-t", "ed25519", "-Z", "aes256-gcm@openssh.com",
        "-a", str(rounds), "-f", str(path), "-C", comment,
    ]


def observe_askpass_failure(binary, root, env, rounds, name, askpass_path):
    key_path = root / (name + "-key")
    askpass_env = dict(env)
    askpass_env["SSH_ASKPASS_REQUIRE"] = "force"
    askpass_env["SSH_ASKPASS"] = str(askpass_path)
    comment = "vivac.device-key.v1:" + identifier() + ":" + identifier()
    code, _ = bounded_run(
        keygen_arguments(binary, key_path, comment, rounds), askpass_env,
    )
    exists = key_path.exists() or key_path.is_symlink()
    observation = {
        "rc": code, "file_exists": exists, "cipher": None, "kdf": None,
        "unsafe_plaintext_created": False, "creation_fail_closed": False,
    }
    require(code >= 0, "askpass failure observation terminated by signal")
    if code == 0:
        require(exists, "successful askpass failure observation produced no key")
        profile = parse_key(key_path)
        observation["cipher"] = profile["cipher"]
        observation["kdf"] = profile["kdf"]
        require(profile["cipher"] == "none", "unexpected askpass failure creation protection")
        observation["unsafe_plaintext_created"] = True
        observation["status"] = "gap_plaintext_created"
    else:
        require(not exists, "failed askpass creation left an unexpected key")
        observation["creation_fail_closed"] = True
        observation["status"] = "refused"
    return observation


def writer_input_observations(writer, root, env, comment):
    invalid = {
        "eof": b"", "empty": b"\n\n", "short": b"short\nshort\n",
        "long": b"a" * 513 + b"\n" + b"a" * 513 + b"\n",
        "mismatch": PASSWORD + b"\n" + WRONG_PASSWORD + b"\n",
        "missing_confirmation": PASSWORD + b"\n",
        "extra_line": PASSWORD + b"\n" + PASSWORD + b"\nextra\n",
        "invalid_utf8": b"\xff" * 16 + b"\n" + b"\xff" * 16 + b"\n",
        "nul": b"a" * 16 + b"\0\n" + b"a" * 16 + b"\0\n",
        "carriage_return": b"a" * 16 + b"\r\n" + b"a" * 16 + b"\r\n",
    }
    for name, fixture in invalid.items():
        destination = root / ("writer-invalid-" + name)
        code, _ = bounded_run([str(writer), str(destination), comment], env, fixture)
        require(code > 0 and not destination.exists(), "writer invalid input was not refused before file creation")
    return list(invalid)


def run_probe(helper, rounds, writer=None):
    executables = {
        name: shutil.which(name, path=os.defpath)
        for name in ("ssh", "ssh-keygen", "ssh-agent", "ssh-add")
    }
    require(all(executables.values()), "required OpenSSH executable is absent")
    report = {
        "version": 1, "interoperability_passed": False,
        "creation_fail_closed": False, "custody_accredited": False,
        "ssh_disconnect_tested": False, "supervisor_crash_tested": False,
        "rounds_experimental": rounds, "observations": {}, "timings_seconds": {},
    }
    with tempfile.TemporaryDirectory(prefix="vivac-openssh-probe-", dir="/tmp") as temporary:
        root = Path(temporary).resolve()
        root.chmod(0o700)
        (root / "home").mkdir(mode=0o700)
        env = environment(root)
        code, version = bounded_run([executables["ssh"], "-V"], env)
        require(code == 0, "OpenSSH version query failed")
        version = version.strip()
        require(
            len(version) <= 256 and re.fullmatch(rb"OpenSSH_[ -~]+", version),
            "OpenSSH version receipt was refused",
        )
        report["provenance"] = {"openssh_version": version.decode("ascii")}
        components = list(executables.items()) + [("proof-verifier", helper)]
        if writer is not None:
            components.append(("native-format-writer", writer))
        for name, executable in components:
            digest = hashlib.sha256()
            with open(executable, "rb") as binary:
                for chunk in iter(lambda: binary.read(1024 * 1024), b""):
                    digest.update(chunk)
            report["provenance"][name.replace("-", "_") + "_sha256"] = digest.hexdigest()

        device_id, credential_id = identifier(), identifier()
        comment = "vivac.device-key.v1:" + device_id + ":" + credential_id
        key_path = root / "device-key"
        started = time.monotonic()
        if writer is None:
            code, _ = pty_run(
                keygen_arguments(executables["ssh-keygen"], key_path, comment, rounds),
                env, [PASSWORD, PASSWORD],
            )
            report["creation_method"] = "native_keygen_pty"
        else:
            require(rounds == 64, "writer experiment has a fixed work factor")
            writer_env = dict(env, VIVAC_OPENSSH_DISPOSABLE="1")
            report["observations"]["writer_invalid_input_refused_before_file_creation"] = writer_input_observations(
                writer, root, writer_env, comment,
            )
            code, _ = bounded_run(
                [str(writer), str(key_path), comment], writer_env,
                PASSWORD + b"\n" + PASSWORD + b"\n",
            )
            report["creation_method"] = "native_format_encoder_fixture_pipe"
        require(code == 0, "encrypted key creation failed")
        parsed = parse_key(key_path)
        require(
            parsed["cipher"] == "aes256-gcm@openssh.com" and parsed["rounds"] == rounds,
            "created key protection differs from the requested profile",
        )
        report["timings_seconds"]["encrypted_create"] = round(time.monotonic() - started, 6)
        report["cipher"] = parsed["cipher"]
        report["kdf"] = parsed["kdf"]

        agent = None
        agent_output = bytearray()
        agent_socket = root / "agent.sock"
        agent_env = environment(root, agent_socket)
        try:
            agent = subprocess.Popen(
                [executables["ssh-agent"], "-D", "-a", str(agent_socket)],
                env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT, start_new_session=True, preexec_fn=child_setup,
            )
            os.set_blocking(agent.stdout.fileno(), False)
            deadline = time.monotonic() + TIMEOUT
            while not agent_socket.exists():
                drain_agent_output(agent, agent_output)
                require(agent.poll() is None, "private agent exited before readiness")
                require(time.monotonic() < deadline, "private agent readiness timed out")
                time.sleep(0.02)
            require(identities(agent_socket) == [], "private agent did not start empty")
            started = time.monotonic()
            code, _ = pty_run(
                [executables["ssh-add"], "-k", str(key_path)], agent_env, [PASSWORD],
            )
            require(code == 0, "encrypted key load failed")
            require(
                identities(agent_socket) == [(parsed["public_blob"], comment.encode("ascii"))],
                "agent identity or comment differs from the generated credential",
            )
            report["timings_seconds"]["encrypted_load"] = round(time.monotonic() - started, 6)
            value = challenge(parsed["public_key"], device_id, credential_id)
            signature = sign(agent_socket, parsed["public_blob"], canonical(value))
            valid_proof = proof(value, signature)
            verify(helper, root, env, valid_proof, "valid-proof", True)
            altered = copy.deepcopy(valid_proof)
            altered["challenge"]["base_revision"] += 1
            verify(helper, root, env, altered, "altered-proof", False)
            old = challenge(parsed["public_key"], device_id, credential_id, expired=True)
            old_proof = proof(old, sign(agent_socket, parsed["public_blob"], canonical(old)))
            verify(helper, root, env, old_proof, "expired-proof", False)
            report["observations"]["exact_proof_verified"] = True
            report["observations"]["altered_and_expired_proofs_refused"] = True

            clear_agent(agent_socket)
            code, output = pty_run(
                [executables["ssh-add"], "-k", str(key_path)], agent_env, [WRONG_PASSWORD],
                allow_retry_eof=True,
            )
            require(
                code != 0 and b"Bad passphrase" in output and identities(agent_socket) == [],
                "wrong passphrase was not explicitly refused",
            )
            report["observations"]["wrong_passphrase_refused"] = True

            clear_agent(agent_socket)
            damaged = root / "damaged-key"
            corrupt_tag(key_path, damaged)
            code, _ = pty_run(
                [executables["ssh-add"], "-k", str(damaged)], agent_env, [PASSWORD],
                require_responses=False, allow_retry_eof=True,
            )
            require(code != 0 and identities(agent_socket) == [], "corrupted tag load was accepted")
            report["observations"]["corrupted_tag_refused"] = True

            clear_agent(agent_socket)
            code, _ = bounded_run([executables["ssh-add"], "-k", str(key_path)], agent_env)
            require(code != 0 and identities(agent_socket) == [], "headless encrypted load was accepted")
            report["observations"]["encrypted_load_without_tty_refused"] = True
            absent = root / "absent-key"
            code, _ = bounded_run([executables["ssh-add"], "-k", str(absent)], agent_env)
            require(
                code != 0 and not absent.exists() and identities(agent_socket) == [],
                "missing key load was not refused",
            )
            report["observations"]["missing_key_refused"] = True
            drain_agent_output(agent, agent_output)
            require(agent.poll() is None, "private agent exited unexpectedly")
            report["interoperability_passed"] = True
        finally:
            stop(agent)
            if agent is not None:
                drain_agent_output(agent, agent_output)
                agent.stdout.close()

        require(agent.poll() is not None and not agent_socket.exists(), "owned agent shutdown was not confirmed")
        report["observations"]["owned_agent_shutdown_confirmed"] = True

        eof_key = root / "eof-key"
        code, _ = bounded_run(
            keygen_arguments(executables["ssh-keygen"], eof_key, "vivac.device-key.v1:" + identifier() + ":" + identifier(), rounds),
            env,
        )
        if code == 0:
            eof_profile = parse_key(eof_key)
            require(eof_profile["cipher"] == "none", "unexpected headless creation result")
            report["observations"]["creation_without_tty"] = "gap_plaintext_created"
        else:
            require(not eof_key.exists(), "failed headless creation left an unexpected key")
            report["observations"]["creation_without_tty"] = "refused"

        empty_key = root / "empty-key"
        code, _ = pty_run(
            keygen_arguments(executables["ssh-keygen"], empty_key, "vivac.device-key.v1:" + identifier() + ":" + identifier(), rounds),
            env, [b"", b""], require_responses=False,
        )
        if code == 0:
            empty_profile = parse_key(empty_key)
            require(empty_profile["cipher"] == "none", "unexpected empty-passphrase creation result")
            report["observations"]["creation_with_empty_passphrase"] = "gap_plaintext_created"
        else:
            require(not empty_key.exists(), "failed empty-passphrase creation left an unexpected key")
            report["observations"]["creation_with_empty_passphrase"] = "refused"

        failing_askpass = root / "failing-askpass"
        private_write(failing_askpass, b"#!/bin/sh\nexit 1\n")
        failing_askpass.chmod(0o700)
        missing_askpass = root / "missing-askpass"
        require(not missing_askpass.exists(), "missing askpass fixture unexpectedly exists")
        for name, askpass_path in (
            ("creation_with_failing_askpass", failing_askpass),
            ("creation_with_missing_askpass", missing_askpass),
        ):
            report["observations"][name] = observe_askpass_failure(
                executables["ssh-keygen"], root, env, rounds, name, askpass_path,
            )

        report["creation_fail_closed"] = all(
            report["observations"][name] == "refused"
            for name in ("creation_without_tty", "creation_with_empty_passphrase")
        ) and all(
            report["observations"][name]["creation_fail_closed"]
            for name in ("creation_with_failing_askpass", "creation_with_missing_askpass")
        )
        return report


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--verifier", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--rounds", type=int, default=64)
    parser.add_argument("--writer", type=Path)
    args = parser.parse_args()
    report_path = None
    try:
        require(sys.platform == "linux", "this probe requires native Linux")
        require(os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1", "disposable-container declaration is absent")
        require(1 <= args.rounds <= 256, "experimental rounds are outside the probe bounds")
        helper = args.verifier.resolve(strict=True)
        require(helper.is_file() and os.access(helper, os.X_OK), "proof verifier is not executable")
        writer = args.writer.resolve(strict=True) if args.writer else None
        if writer is not None:
            require(writer.is_file() and os.access(writer, os.X_OK), "writer is not executable")
        require(args.report.is_absolute(), "report path must be absolute")
        report_path = args.report.parent.resolve(strict=True) / args.report.name
        require(not report_path.exists() and not report_path.is_symlink(), "report destination already exists")
        report = run_probe(helper, args.rounds, writer)
        private_write(report_path, json.dumps(report, indent=2, sort_keys=True).encode("utf-8") + b"\n")
        print(
            "OpenSSH observations: interoperability_passed="
            + str(report["interoperability_passed"]).lower()
            + " creation_fail_closed=" + str(report["creation_fail_closed"]).lower()
            + " custody_accredited=false",
            flush=True,
        )
        return 0
    except (ProbeError, OSError, ValueError, subprocess.SubprocessError) as error:
        message = str(error) if isinstance(error, ProbeError) else "native operation or report I/O failed"
        if report_path is not None and not report_path.exists() and not report_path.is_symlink():
            failure = {
                "version": 1, "status": "incomplete", "failure": message,
                "interoperability_passed": False, "creation_fail_closed": False,
                "custody_accredited": False,
            }
            try:
                private_write(report_path, json.dumps(failure, sort_keys=True).encode("utf-8") + b"\n")
            except OSError:
                pass
        print("OpenSSH probe failed: " + message + "; no result is accredited.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
