#!/usr/bin/env python3
"""Keep one simulated WebAuthn authenticator alive across create and get."""

import json
import os
import signal
import sys
import tempfile
import time

import registration_browser_driver as registration


def interrupted(_number, _frame):
    raise registration.DriverFailure("global_time_limit_or_parent_exit")


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        registration.require(key not in result, "duplicate_input_field")
        result[key] = value
    return result


def read_request(action):
    raw = sys.stdin.buffer.readline(registration.MAX_INPUT + 1)
    registration.require(len(raw) <= registration.MAX_INPUT, "input_line_too_large")
    registration.require(raw.endswith(b"\n"), "input_line_missing")
    try:
        request = json.loads(raw.decode("utf-8"), object_pairs_hook=unique_object)
    except (UnicodeDecodeError, json.JSONDecodeError):
        raise registration.DriverFailure("invalid_input_json") from None
    registration.require(isinstance(request, dict), "input_must_be_object")
    expected = {"version", "action", "options"}
    if action == "assert":
        expected.add("verified")
    registration.require(set(request) == expected, "unexpected_input_fields")
    registration.require(type(request["version"]) is int and request["version"] == 1,
                         "unsupported_input_version")
    registration.require(request["action"] == action, "unexpected_input_action")
    return request


def emit(value):
    raw = json.dumps(value, separators=(",", ":"), ensure_ascii=True).encode("ascii") + b"\n"
    registration.require(len(raw) <= registration.MAX_OUTPUT, "output_line_too_large")
    sys.stdout.buffer.write(raw)
    sys.stdout.buffer.flush()


def evaluate_assertion(options):
    registration.require(isinstance(options, dict), "assertion_options_must_be_object")
    registration.require(options.get("rpId") == "vivac.test", "assertion_rp_must_match_fixture")
    registration.require(options.get("userVerification") == "required",
                         "assertion_user_verification_must_be_required")
    allowed = options.get("allowCredentials")
    registration.require(isinstance(allowed, list) and len(allowed) == 1,
                         "assertion_must_select_registered_key")
    registration.require("extensions" not in options or options["extensions"] in (None, {}),
                         "assertion_extensions_not_supported")
    script = r"""const done=arguments[arguments.length-1];
try {
  const opts=PublicKeyCredential.parseRequestOptionsFromJSON(arguments[0]);
  const controller=new AbortController();
  const timer=setTimeout(()=>controller.abort(),5000);
  navigator.credentials.get({publicKey:opts,signal:controller.signal}).then(c=>{
    clearTimeout(timer);
    const n=c.toJSON();
    const top=new Set(['id','rawId','type','response','authenticatorAttachment','clientExtensionResults']);
    const fields=new Set(['clientDataJSON','authenticatorData','signature','userHandle']);
    const own=(o,k)=>Object.prototype.hasOwnProperty.call(o,k);
    if(Object.keys(n).some(k=>!top.has(k)) || Object.keys(n.response||{}).some(k=>!fields.has(k))) {
      done({kind:'unexpected-native-fields'}); return;
    }
    if(['id','rawId','type','response'].some(k=>!own(n,k))) {
      done({kind:'native-shape-invalid'}); return;
    }
    const r=n.response;
    if(!r || ['clientDataJSON','authenticatorData','signature'].some(k=>typeof r[k]!=='string')) {
      done({kind:'native-shape-invalid'}); return;
    }
    const wire={id:n.id,rawId:n.rawId,type:n.type,response:{
      clientDataJSON:r.clientDataJSON,authenticatorData:r.authenticatorData,signature:r.signature}};
    if(own(r,'userHandle')) wire.response.userHandle=r.userHandle;
    if(own(n,'authenticatorAttachment')) wire.authenticatorAttachment=n.authenticatorAttachment;
    if(own(n,'clientExtensionResults')) wire.clientExtensionResults=n.clientExtensionResults;
    done({kind:'asserted',native:n,wire:wire});
  },e=>{clearTimeout(timer);done({kind:'refused',name:e&&e.name||'Error'});});
} catch(e) { done({kind:'refused',name:e&&e.name||'Error'}); }
"""
    _, result = registration.webdriver("POST", "/session/{}/execute/async".format(registration._SESSION),
                                       {"script": script, "args": [options]}, maximum=16)
    registration.require(isinstance(result, dict), "assertion_result_invalid")
    return result


def main():
    registration._DEADLINE = time.monotonic() + 90
    signal.signal(signal.SIGALRM, interrupted)
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    signal.alarm(90)
    directory = None
    try:
        request = read_request("register")
        registration.validate_options(request["options"])
        evidence = registration.check_runtime()
        directory = tempfile.mkdtemp(prefix="r-", dir=registration.DATA)
        os.chmod(directory, 0o700)
        common = registration.start_browser(directory, "verified", evidence)
        created = registration.evaluate_creation(request["options"], "verified")
        emit(dict(common, status="created", native=created["native"], wire=created["wire"]))
        request = read_request("assert")
        registration.require(type(request["verified"]) is bool, "verified_must_be_boolean")
        if not request["verified"]:
            registration.webdriver("POST", "/session/{}/goog/cdp/execute".format(registration._SESSION),
                                   {"cmd": "WebAuthn.setUserVerified", "params": {
                                       "authenticatorId": registration._AUTHENTICATOR,
                                       "isUserVerified": False}})
        assertion = evaluate_assertion(request["options"])
        if request["verified"]:
            registration.require(assertion.get("kind") == "asserted",
                                 "verified_assertion_failed_" + str(assertion.get("name", assertion.get("kind"))))
            response = dict(common, status="asserted", native=assertion["native"], wire=assertion["wire"])
        else:
            registration.require(assertion.get("kind") == "refused" and
                                 assertion.get("name") in ("NotAllowedError", "AbortError"),
                                 "unverified_assertion_did_not_refuse")
            response = dict(common, status="browser-refused", error_name=assertion["name"])
        emit(response)
        registration.require(sys.stdin.buffer.read(1) == b"", "extra_input_line")
    finally:
        # Cleanup gets its own short allowance after the ceremony deadline.
        signal.alarm(0)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        registration._DEADLINE = time.monotonic() + 8
        if directory is not None:
            registration.cleanup(directory, strict=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        message = str(exc) if isinstance(exc, registration.DriverFailure) else "driver_internal_" + type(exc).__name__
        print("Isolated final confirmation failed: " + message, file=sys.stderr)
        raise SystemExit(1)
