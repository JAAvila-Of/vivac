#!/usr/bin/env python3
"""Run one WebAuthn registration ceremony in the isolated browser lab."""

import base64
import hashlib
import http.server
import json
import os
import re
import shutil
import signal
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request


CHROME = "/assets/chrome-linux64/chrome"
CHROMEDRIVER = "/assets/chromedriver-linux64/chromedriver"
SYSROOT = ("/assets-ubuntu/sysroot/usr/lib/x86_64-linux-gnu:"
           "/assets-ubuntu/sysroot/lib/x86_64-linux-gnu:"
           "/assets/sysroot/usr/lib/x86_64-linux-gnu:/assets/sysroot/lib/x86_64-linux-gnu")
DATA = "/test-data/lab"
BASE_URL = "http://127.0.0.1:9515"
ORIGIN = "https://vivac.test:8443"
MAX_INPUT = 32 * 1024
MAX_OUTPUT = 64 * 1024
LIMIT_SECONDS = 60
_DRIVER = None
_SESSION = None
_AUTHENTICATOR = None
_SERVER = None
_DEADLINE = None


class DriverFailure(Exception):
    pass


def require(condition, message):
    if not condition:
        raise DriverFailure(message)


def remaining():
    value = _DEADLINE - time.monotonic()
    require(value > 0, "global_time_limit_exceeded")
    return value


def read_text(path, limit=8192):
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as stream:
            return stream.read(limit)
    except OSError:
        return ""


def check_runtime():
    require(os.name == "posix" and read_text("/proc/sys/kernel/ostype", 64).strip() == "Linux",
            "linux_required")
    require(os.environ.get("VIVAC_BROWSER_LAB") == "isolated-v1", "isolated_runtime_marker_missing")
    require(os.getuid() == 65534, "uid_must_be_65534")
    status = read_text("/proc/self/status", 16384)
    cap_eff = re.search(r"^CapEff:\s*([0-9a-fA-F]+)$", status, re.MULTILINE)
    require(cap_eff is not None and int(cap_eff.group(1), 16) == 0, "capabilities_must_be_dropped")
    require(re.search(r"^NoNewPrivs:\s*1$", status, re.MULTILINE) is not None,
            "no_new_privileges_required")
    require(os.path.isdir(DATA) and os.access(DATA, os.W_OK | os.X_OK), "lab_data_not_writable")
    st = os.stat(DATA)
    require(st.st_uid == 65534 and st.st_mode & 0o777 == 0o700, "lab_data_permissions_invalid")
    mounts = {}
    for row in read_text("/proc/self/mountinfo", 1024 * 1024).splitlines():
        fields = row.split()
        if len(fields) >= 6:
            mounts[fields[4].replace("\\040", " ")] = fields[5].split(",")
    require("ro" in mounts.get("/assets", []), "assets_mount_not_read_only")
    if os.path.isdir("/assets-ubuntu"):
        require("ro" in mounts.get("/assets-ubuntu", []), "ubuntu_libraries_not_read_only")
    require("ro" in mounts.get("/", []), "root_mount_not_read_only")
    require("ro" in mounts.get("/browser-lab", []), "source_mount_not_read_only")
    require("rw" in mounts.get("/test-data", []), "test_data_mount_not_writable")
    for path in (CHROME, CHROMEDRIVER):
        require(os.path.isfile(path) and os.access(path, os.X_OK), "required_asset_unavailable")
        linked = subprocess.run(["/usr/bin/ldd", path],
                                env=dict(os.environ, LD_LIBRARY_PATH=SYSROOT),
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                timeout=min(5, remaining()), check=False)
        missing = [line.strip() for line in linked.stdout.decode("utf-8", errors="replace").splitlines()
                   if "not found" in line]
        require(linked.returncode == 0 and not missing,
                "missing_browser_libraries: " + "; ".join(missing))
    try:
        os.chroot(DATA)
    except PermissionError:
        outer_chroot_refused = True
    else:
        raise DriverFailure("outer_chroot_unexpectedly_allowed")
    preflight = subprocess.run(
        ["/usr/bin/unshare", "--user", "--map-root-user", "--pid", "--net", "--fork", "/bin/true"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=min(5, remaining()), check=False)
    require(preflight.returncode == 0, "namespace_preflight_failed")
    return {"namespace": True, "seccomp_bpf": False, "outer_chroot_refused": outer_chroot_refused}


def webdriver(method, path, payload=None, maximum=10):
    body = None if payload is None else json.dumps(payload, separators=(",", ":")).encode()
    req = urllib.request.Request(BASE_URL + path, data=body,
                                 headers={"Content-Type": "application/json"} if body is not None else {},
                                 method=method)
    try:
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        with opener.open(req, timeout=min(maximum, remaining())) as response:
            raw = response.read(1024 * 1024)
    except urllib.error.HTTPError as exc:
        detail = exc.read(8192)
        try:
            failure = json.loads(detail).get("value", {})
            message = str(failure.get("error", "")) + ": " + str(failure.get("message", ""))[:1000]
        except (ValueError, AttributeError):
            message = "unreadable_error"
        raise DriverFailure("webdriver_http_" + str(exc.code) + " at " + path + ": " + message) from None
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        raise DriverFailure("webdriver_transport_" + type(exc).__name__) from None
    try:
        value = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        raise DriverFailure("webdriver_invalid_json") from None
    result = value.get("value")
    if isinstance(result, dict) and result.get("error"):
        raise DriverFailure("webdriver_command_" + str(result.get("error")))
    return value, result


def run_openssl(args, *, output=True):
    try:
        result = subprocess.run(["/usr/bin/openssl"] + args, stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, timeout=min(10, remaining()), check=False)
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise DriverFailure("openssl_" + type(exc).__name__) from None
    require(result.returncode == 0, "openssl_failed")
    return result.stdout if output else b""


class FixtureHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = ("<!doctype html><meta charset=utf-8><title>Registration fixture</title>"
                "<script>document.documentElement.dataset.secure=String(isSecureContext)</script>").encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        return


def start_fixture(directory):
    key = os.path.join(directory, "fixture-key.pem")
    cert = os.path.join(directory, "fixture-cert.pem")
    run_openssl(["req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256",
                 "-nodes", "-days", "1", "-subj", "/CN=vivac.test", "-addext",
                 "subjectAltName=DNS:vivac.test", "-keyout", key, "-out", cert], output=False)
    der = run_openssl(["x509", "-in", cert, "-pubkey", "-noout"])
    # Pipe the public key through openssl without exposing it in a file.
    pub = subprocess.run(["/usr/bin/openssl", "pkey", "-pubin", "-outform", "DER"],
                         input=der, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                         timeout=min(10, remaining()), check=False)
    require(pub.returncode == 0, "openssl_spki_failed")
    pin = base64.b64encode(hashlib.sha256(pub.stdout).digest()).decode("ascii")
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 8443), FixtureHandler)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert, key)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, pin


def validate_options(options):
    require(isinstance(options, dict), "options_must_be_object")
    require(isinstance(options.get("rp"), dict) and options["rp"].get("id") == "vivac.test",
            "rp_id_must_match_fixture")
    selection = options.get("authenticatorSelection")
    require(isinstance(selection, dict) and selection.get("userVerification") == "required",
            "user_verification_must_be_required")
    require(options.get("attestation") == "none", "attestation_must_be_none")
    require("extensions" not in options or options["extensions"] in (None, {}),
            "extensions_not_supported")


def evaluate_creation(options, scenario):
    script = r"""const done=arguments[arguments.length-1];
try {
  const opts=PublicKeyCredential.parseCreationOptionsFromJSON(arguments[0]);
  const controller=new AbortController();
  const timer=setTimeout(()=>controller.abort(),5000);
  navigator.credentials.create({publicKey:opts,signal:controller.signal}).then(c=>{
    clearTimeout(timer);
    const n=c.toJSON();
    const allowedTop=new Set(['id','rawId','type','authenticatorAttachment','clientExtensionResults','response']);
    const allowedResponse=new Set(['clientDataJSON','attestationObject','transports','publicKey','publicKeyAlgorithm','authenticatorData']);
    if(Object.keys(n).some(k=>!allowedTop.has(k)) || Object.keys(n.response||{}).some(k=>!allowedResponse.has(k))) {
      done({kind:'unexpected-native-fields'}); return;
    }
    const required=['id','type','rawId','response'];
    if(required.some(k=>!Object.prototype.hasOwnProperty.call(n,k))) { done({kind:'native-shape-invalid'}); return; }
    const r=n.response;
    if(!r || typeof r.clientDataJSON!=='string' || typeof r.attestationObject!=='string') {
      done({kind:'native-shape-invalid'}); return;
    }
    const wire={id:n.id,type:n.type,rawId:n.rawId,response:{
      clientDataJSON:r.clientDataJSON,attestationObject:r.attestationObject}};
    if(Object.prototype.hasOwnProperty.call(r,'transports')) wire.response.transports=r.transports;
    if(Object.prototype.hasOwnProperty.call(n,'authenticatorAttachment')) wire.authenticatorAttachment=n.authenticatorAttachment;
    if(Object.prototype.hasOwnProperty.call(n,'clientExtensionResults')) wire.clientExtensionResults=n.clientExtensionResults;
    done({kind:'created',native:n,wire:wire});
  },e=>{clearTimeout(timer);done({kind:'refused',name:e&&e.name||'Error'});});
} catch(e) { done({kind:'refused',name:e&&e.name||'Error'}); }
"""
    _, result = webdriver("POST", "/session/{}/execute/async".format(_SESSION),
                          {"script": script, "args": [options]}, maximum=16)
    require(isinstance(result, dict), "creation_result_invalid")
    if scenario == "verified":
        require(result.get("kind") == "created", "verified_creation_failed_" + str(result.get("name", result.get("kind"))))
        return result
    require(result.get("kind") == "refused" and result.get("name") in ("NotAllowedError", "AbortError"),
            "unverified_browser_did_not_refuse")
    return result


def stop_driver(*, strict=False):
    global _DRIVER
    if _DRIVER is None:
        return
    try:
        os.killpg(_DRIVER.pid, signal.SIGTERM)
    except OSError:
        pass
    try:
        _DRIVER.wait(timeout=min(2, max(0.1, remaining())))
    except (subprocess.TimeoutExpired, DriverFailure):
        try:
            os.killpg(_DRIVER.pid, signal.SIGKILL)
        except OSError:
            pass
        try:
            _DRIVER.wait(timeout=1)
        except subprocess.TimeoutExpired:
            pass
    reaped = _DRIVER.poll() is not None
    _DRIVER = None
    if strict:
        require(reaped, "chromedriver_not_reaped")


def private_environment(directory):
    env = os.environ.copy()
    env.update({"LD_LIBRARY_PATH": SYSROOT, "HOME": directory, "TMPDIR": directory,
                "CHROME_LOG_FILE": os.path.join(directory, "chrome.log")})
    font_root = "/assets-ubuntu/sysroot/usr/share/fonts"
    if os.path.isdir(font_root):
        font_cache = os.path.join(directory, "font-cache")
        os.mkdir(font_cache, 0o700)
        font_config = os.path.join(directory, "fonts.conf")
        with open(font_config, "w", encoding="utf-8") as output:
            output.write("<fontconfig><dir>" + font_root + "</dir><cachedir>" + font_cache
                         + "</cachedir></fontconfig>\n")
        env["FONTCONFIG_FILE"] = font_config
        env["FONTCONFIG_PATH"] = directory
    return env


def start_browser(directory, scenario, evidence):
    global _DRIVER, _SESSION, _AUTHENTICATOR, _SERVER
    env = private_environment(directory)
    _SERVER, pin = start_fixture(directory)
    _DRIVER = subprocess.Popen([CHROMEDRIVER, "--port=9515", "--allowed-ips=127.0.0.1",
                                "--log-path=" + os.path.join(directory, "chromedriver.log")],
                                cwd=directory, env=env, stdin=subprocess.DEVNULL,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                start_new_session=True, close_fds=True)
    while True:
        remaining()
        require(_DRIVER.poll() is None, "chromedriver_exited_early")
        try:
            _, status = webdriver("GET", "/status", maximum=2)
            if status and status.get("ready") is True:
                break
        except DriverFailure:
            time.sleep(min(0.1, remaining()))
    caps = {"capabilities": {"alwaysMatch": {"browserName": "chrome", "goog:chromeOptions": {
        "binary": CHROME, "args": ["--headless=new", "--user-data-dir=" + os.path.join(directory, "profile"),
            "--no-first-run", "--no-default-browser-check", "--disable-background-networking",
            "--disable-sync", "--disable-component-update", "--disable-gpu",
            "--renderer-process-limit=1", "--ignore-certificate-errors-spki-list=" + pin,
            "--host-resolver-rules=MAP vivac.test 127.0.0.1"]}}}}
    created, value = webdriver("POST", "/session", caps, maximum=30)
    _SESSION = value.get("sessionId") or created.get("sessionId")
    require(_SESSION, "webdriver_session_missing")
    browser_version = value.get("capabilities", {}).get("browserVersion")
    require(browser_version == "157.0.8103.78", "unexpected_browser_version")
    webdriver("POST", "/session/{}/timeouts".format(_SESSION), {"script": 15000})
    _, _ = webdriver("POST", "/session/{}/goog/cdp/execute".format(_SESSION),
                     {"cmd": "WebAuthn.enable", "params": {}}, maximum=10)
    auth, _ = webdriver("POST", "/session/{}/goog/cdp/execute".format(_SESSION),
                        {"cmd": "WebAuthn.addVirtualAuthenticator", "params": {"options": {
                            "protocol": "ctap2", "transport": "usb", "hasResidentKey": False,
                            "hasUserVerification": True, "isUserVerified": scenario == "verified",
                            "automaticPresenceSimulation": True}}}, maximum=10)
    _AUTHENTICATOR = auth.get("value", {}).get("authenticatorId")
    require(_AUTHENTICATOR, "virtual_authenticator_missing")
    webdriver("POST", "/session/{}/url".format(_SESSION), {"url": ORIGIN}, maximum=10)
    _, page = webdriver("POST", "/session/{}/execute/sync".format(_SESSION),
                         {"script": "return {secure: isSecureContext, origin: location.origin};", "args": []})
    require(page == {"secure": True, "origin": ORIGIN}, "fixture_origin_not_secure")
    sandbox_script = "return document.body ? document.body.innerText : '';"
    webdriver("POST", "/session/{}/url".format(_SESSION), {"url": "chrome://sandbox"})
    _, sandbox = webdriver("POST", "/session/{}/execute/sync".format(_SESSION),
                            {"script": sandbox_script, "args": []})
    require(isinstance(sandbox, str), "sandbox_evidence_unavailable")
    namespace = (re.search(r"Layer 1 Sandbox\s+Namespace", sandbox) is not None and
                 re.search(r"PID namespaces\s+Yes", sandbox) is not None and
                 re.search(r"Network namespaces\s+Yes", sandbox) is not None)
    seccomp = re.search(r"Seccomp-BPF sandbox\s+Yes", sandbox) is not None
    require(namespace, "namespace_sandbox_not_enabled")
    require(seccomp, "seccomp_bpf_sandbox_not_enabled")
    evidence.update({"namespace": True, "seccomp_bpf": True})
    webdriver("POST", "/session/{}/url".format(_SESSION), {"url": ORIGIN})
    return {"version": 1, "evidence": {"sandbox": evidence, "browser_version": browser_version,
                "origin": ORIGIN, "secure_context": True, "virtual_authenticator": True}}


def cleanup(directory, *, strict=False):
    global _SESSION, _AUTHENTICATOR, _SERVER
    failures = []
    if _SESSION:
        if _AUTHENTICATOR:
            try:
                webdriver("POST", "/session/{}/goog/cdp/execute".format(_SESSION),
                          {"cmd": "WebAuthn.removeVirtualAuthenticator", "params":
                           {"authenticatorId": _AUTHENTICATOR}}, maximum=2)
            except Exception:
                failures.append("virtual_authenticator_cleanup_failed")
        try:
            webdriver("DELETE", "/session/" + _SESSION, maximum=2)
        except Exception:
            failures.append("webdriver_session_cleanup_failed")
    _SESSION = _AUTHENTICATOR = None
    try:
        stop_driver(strict=strict)
    except Exception:
        failures.append("chromedriver_cleanup_failed")
    if _SERVER is not None:
        _SERVER.shutdown()
        _SERVER.server_close()
        _SERVER = None
    try:
        shutil.rmtree(directory, ignore_errors=not strict)
    except OSError:
        failures.append("private_directory_cleanup_failed")
    if strict:
        require(not failures, ";".join(failures))


def main():
    global _DRIVER, _SESSION, _AUTHENTICATOR, _SERVER, _DEADLINE
    _DEADLINE = time.monotonic() + LIMIT_SECONDS
    raw = sys.stdin.buffer.read(MAX_INPUT + 1)
    require(len(raw) <= MAX_INPUT, "input_too_large")
    try:
        request = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        raise DriverFailure("invalid_input_json") from None
    require(isinstance(request, dict) and request.get("version") == 1, "unsupported_input_version")
    require(set(request) == {"version", "scenario", "options"}, "unexpected_input_fields")
    scenario = request.get("scenario")
    require(scenario in ("verified", "unverified"), "invalid_scenario")
    options = request.get("options")
    validate_options(options)
    evidence = check_runtime()
    # Chrome creates Unix sockets beneath TMPDIR, whose address length is bounded.
    directory = tempfile.mkdtemp(prefix="r-", dir=DATA)
    os.chmod(directory, 0o700)
    try:
        common = start_browser(directory, scenario, evidence)
        result = evaluate_creation(options, scenario)
        if result["kind"] == "created":
            encoded = result["wire"]["response"]["clientDataJSON"]
            client = json.loads(base64.urlsafe_b64decode(encoded + "=" * (-len(encoded) % 4)))
            print("Browser registration client-data fields: " + ", ".join(sorted(client)), file=sys.stderr)
        if result["kind"] == "created":
            common.update({"status": "created", "native": result["native"], "wire": result["wire"]})
        else:
            common.update({"status": "browser-refused", "error_name": result["name"]})
        return common
    finally:
        if sys.exc_info()[0] is not None:
            for filename in ("chrome.log", "chromedriver.log"):
                diagnostic = read_text(os.path.join(directory, filename), 16384)
                if diagnostic:
                    print("Browser lab diagnostic " + filename + ": " + diagnostic[-2500:], file=sys.stderr)
        cleanup(directory)


def emit(value, success):
    data = json.dumps(value, separators=(",", ":"), ensure_ascii=True).encode("utf-8")
    if len(data) > MAX_OUTPUT:
        value = {"version": 1, "status": "error", "error": "output_too_large"}
        data = json.dumps(value, separators=(",", ":")).encode("ascii")
        success = False
    sys.stdout.buffer.write(data + b"\n")
    return 0 if success else 1


if __name__ == "__main__":
    try:
        response = main()
        raise SystemExit(emit(response, True))
    except Exception as exc:
        message = str(exc) if isinstance(exc, DriverFailure) else "driver_internal_" + type(exc).__name__
        print("Isolated browser registration failed: " + message, file=sys.stderr)
        raise SystemExit(emit({"version": 1, "status": "error", "error": message}, False))
