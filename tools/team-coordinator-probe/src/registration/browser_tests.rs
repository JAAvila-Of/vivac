use super::*;
use serde_json::{json, Value as Json};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

const RP: &str = "vivac.test";
const ORIGIN: &str = "https://vivac.test:8443";
const REVIEW: [u8; 32] = [0x52; 32];
const DRIVER: &str = "/browser-lab/registration_browser_driver.py";
const MAX_DRIVER_INPUT: usize = 32 * 1024;
const MAX_DRIVER_BYTES: usize = 64 * 1024;
const DRIVER_TIMEOUT: Duration = Duration::from_secs(90);

fn public_proof() -> Proof {
    let now = crate::enrollment_proof::current_time().unwrap();
    // RFC 8032, section 7.1, test 1: this published seed is only a fixture.
    let signing = ed25519_dalek::SigningKey::from_bytes(
        &crate::enrollment_proof::decode_hex::<32>(
            "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        )
        .unwrap(),
    );
    Proof::signed(
        crate::enrollment_proof::Challenge {
            version: 1,
            organization_id: "00000000000000000000000001".into(),
            principal_id: "00000000000000000000000003".into(),
            device_id: "00000000000000000000000004".into(),
            credential_id: "00000000000000000000000005".into(),
            public_key: signing
                .verifying_key()
                .to_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            operation_id: "00000000000000000000000006".into(),
            base_revision: 7,
            nonce: "23".repeat(32),
            issued_at: now,
            expires_at: now + 300,
        },
        &signing,
        now,
    )
    .unwrap()
}

fn fixture() -> (
    RegistrationVerifier,
    Proof,
    RegistrationRequest,
    PendingRegistration,
) {
    assert_eq!(
        std::env::var("VIVAC_BROWSER_LAB").as_deref(),
        Ok("isolated-v1"),
        "browser tests require the isolated lab"
    );
    assert!(Path::new(DRIVER).is_file(), "browser lab driver is missing");
    let proof = public_proof();
    let verifier = RegistrationVerifier::new(RP, ORIGIN).unwrap();
    // Unsupported CPUs must fail this lab, never turn a browser check into a skip.
    let (request, pending) = verifier.start(&proof, REVIEW).unwrap();
    (verifier, proof, request, pending)
}

fn browser(request: &RegistrationRequest, scenario: &str) -> Json {
    let input = serde_json::to_vec(&json!({
        "version": 1,
        "scenario": scenario,
        "options": request.options,
    }))
    .unwrap();
    assert!(input.len() <= MAX_DRIVER_INPUT);
    let deadline = Instant::now() + DRIVER_TIMEOUT;
    let mut child = Command::new("/usr/bin/python3")
        .arg(DRIVER)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start isolated browser lab driver");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (input_tx, input_rx) = mpsc::channel();
    thread::spawn(move || {
        let result = stdin.write_all(&input);
        drop(stdin);
        let _ = input_tx.send(result);
    });
    let (output_tx, output_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take((MAX_DRIVER_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = output_tx.send(result);
    });
    if !matches!(
        input_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())),
        Ok(Ok(()))
    ) {
        let _ = child.kill();
        let _ = child.wait();
        panic!("browser lab request failed or stdin did not close before the deadline");
    }
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll browser lab driver") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("browser lab driver timed out");
        }
        thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "browser lab driver failed: {status}");
    let output = output_rx
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .expect("browser lab stdout did not close before the deadline")
        .expect("read browser lab response");
    assert!(
        output.len() <= MAX_DRIVER_BYTES,
        "browser lab output is oversized"
    );
    let result: Json = serde_json::from_slice(&output).expect("browser lab must return JSON");
    assert_eq!(result["version"], 1);
    for field in ["namespace", "seccomp_bpf", "outer_chroot_refused"] {
        assert_eq!(result["evidence"]["sandbox"][field], true);
    }
    assert_eq!(result["evidence"]["origin"], ORIGIN);
    assert_eq!(result["evidence"]["secure_context"], true);
    assert_eq!(result["evidence"]["virtual_authenticator"], true);
    assert!(result["evidence"]["browser_version"]
        .as_str()
        .is_some_and(|version| !version.is_empty()));
    result
}

fn assert_fields(value: &Json, allowed: &[&str]) {
    for field in value
        .as_object()
        .expect("expected a browser JSON object")
        .keys()
    {
        assert!(
            allowed.contains(&field.as_str()),
            "unknown browser field: {field}"
        );
    }
}

fn verified_response(request: &RegistrationRequest) -> (Json, Vec<u8>) {
    let result = browser(request, "verified");
    assert_eq!(result["status"], "created");
    let native = &result["native"];
    let wire = &result["wire"];
    assert_fields(
        native,
        &[
            "id",
            "rawId",
            "type",
            "response",
            "authenticatorAttachment",
            "clientExtensionResults",
        ],
    );
    assert_fields(
        &native["response"],
        &[
            "clientDataJSON",
            "attestationObject",
            "transports",
            "publicKey",
            "publicKeyAlgorithm",
            "authenticatorData",
        ],
    );
    let mut expected = native.clone();
    let response = expected["response"].as_object_mut().unwrap();
    // The minimal DTO omits only these derived Level 3 response fields.
    for field in ["publicKey", "publicKeyAlgorithm", "authenticatorData"] {
        response.remove(field);
    }
    assert_eq!(
        &expected, wire,
        "the adapter must preserve all other native fields"
    );
    assert_eq!(wire["clientExtensionResults"], json!({}));
    assert!(wire["authenticatorAttachment"].as_str().is_some());
    assert!(wire["response"]["transports"].as_array().is_some());
    assert_eq!(wire["id"], wire["rawId"]);
    assert_eq!(wire["type"], "public-key");
    let raw = serde_json::to_vec(wire).unwrap();
    (result, raw)
}

#[test]
fn browser_real_registration_accepts_metadata() {
    let (verifier, proof, request, pending) = fixture();
    let (browser, raw) = verified_response(&request);
    let result = verifier.finish(pending, &proof, REVIEW, &raw).unwrap();
    let descriptor = result.descriptor();
    assert!(matches!(descriptor.algorithm, -7 | -8));
    assert_eq!(
        descriptor.credential,
        Base64UrlUnpadded::decode_vec(browser["wire"]["rawId"].as_str().unwrap()).unwrap()
    );
    assert_eq!(
        json!(descriptor.transports),
        browser["native"]["response"]["transports"]
    );
    assert_eq!(
        json!(descriptor.attachment),
        browser["native"]["authenticatorAttachment"]
    );
    assert_eq!(
        descriptor.opaque_user_handle,
        proof.challenge.principal_id.as_bytes()
    );
    assert_eq!(result.response, raw);
    assert_eq!(
        serde_json::to_value(result.context).unwrap(),
        serde_json::to_value(request.context).unwrap()
    );
    // Independently assemble the specified record preimage from public inputs.
    let mut record = b"vivac.passkey.registration-record.v1\0".to_vec();
    record.extend(Base64UrlUnpadded::decode_vec(&request.options.challenge).unwrap());
    record.extend((raw.len() as u32).to_be_bytes());
    record.extend(&raw);
    let expected: [u8; 32] = Sha256::digest(&record).into();
    assert_eq!(result.commitment(), expected);
    record.push(0);
    let changed: [u8; 32] = Sha256::digest(&record).into();
    assert_ne!(result.commitment(), changed);
}

#[test]
fn browser_changed_origin_is_refused() {
    let (verifier, proof, request, pending) = fixture();
    let (browser, _) = verified_response(&request);
    let mut wire = browser["wire"].clone();
    let client_bytes =
        Base64UrlUnpadded::decode_vec(wire["response"]["clientDataJSON"].as_str().unwrap())
            .unwrap();
    let mut client: Json = serde_json::from_slice(&client_bytes).unwrap();
    assert_eq!(client["origin"], ORIGIN);
    client["origin"] = json!("https://other.test");
    wire["response"]["clientDataJSON"] = json!(Base64UrlUnpadded::encode_string(
        &serde_json::to_vec(&client).unwrap()
    ));
    assert!(matches!(
        verifier.finish(pending, &proof, REVIEW, &serde_json::to_vec(&wire).unwrap()),
        Err(RegistrationError::Refused)
    ));
}

#[test]
fn browser_changed_proposal_is_refused() {
    let (verifier, proof, request, pending) = fixture();
    let (_, raw) = verified_response(&request);
    assert!(matches!(
        verifier.finish(pending, &proof, [0x53; 32], &raw),
        Err(RegistrationError::Refused)
    ));
}

#[test]
fn browser_without_user_verification_is_refused() {
    let (verifier, proof, request, pending) = fixture();
    let browser = browser(&request, "unverified");
    assert_eq!(browser["status"], "browser-refused");
    assert!(matches!(
        browser["error_name"].as_str(),
        Some("NotAllowedError" | "AbortError")
    ));
    assert!(browser.get("native").is_none());
    assert!(browser.get("wire").is_none());
    assert!(matches!(
        verifier.finish(pending, &proof, REVIEW, b""),
        Err(RegistrationError::Refused)
    ));
}

const FINAL_DRIVER: &str = "/browser-lab/final_confirmation_browser_driver.py";

struct ConfirmationBrowser {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    output: mpsc::Receiver<std::io::Result<Vec<u8>>>,
    deadline: Instant,
}

impl ConfirmationBrowser {
    fn start() -> Self {
        assert!(Path::new(FINAL_DRIVER).is_file());
        let mut child = Command::new("/usr/bin/python3")
            .arg(FINAL_DRIVER)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start final confirmation browser driver");
        let stdin = child.stdin.take();
        let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let (sender, output) = mpsc::channel();
        thread::spawn(move || {
            for _ in 0..3 {
                let result = (|| {
                    let mut line = Vec::new();
                    let mut byte = [0];
                    while stdout.read(&mut byte)? != 0 {
                        line.push(byte[0]);
                        if line.len() > MAX_DRIVER_BYTES {
                            return Err(std::io::Error::other("oversized browser output line"));
                        }
                        if byte[0] == b'\n' {
                            return Ok(line);
                        }
                    }
                    if !line.is_empty() {
                        return Err(std::io::Error::other("unterminated browser output line"));
                    }
                    Ok(line)
                })();
                let stop = result.as_ref().map_or(true, Vec::is_empty);
                if sender.send(result).is_err() || stop {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            output,
            deadline: Instant::now() + Duration::from_secs(120),
        }
    }

    fn timeout(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    fn exchange(&mut self, request: Json) -> Json {
        let mut input = serde_json::to_vec(&request).unwrap();
        input.push(b'\n');
        assert!(input.len() <= MAX_DRIVER_INPUT);
        let mut stdin = self.stdin.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = stdin.write_all(&input).and_then(|()| stdin.flush());
            let _ = sender.send((stdin, result));
        });
        let (stdin, result) = receiver
            .recv_timeout(self.timeout())
            .expect("browser input deadline");
        self.stdin = Some(stdin);
        result.expect("write browser request");
        let line = self
            .output
            .recv_timeout(self.timeout())
            .expect("browser response deadline")
            .expect("read browser response");
        assert!(!line.is_empty(), "browser closed before responding");
        let result: Json = serde_json::from_slice(&line).expect("browser response JSON");
        assert_eq!(result["version"], 1);
        for field in ["namespace", "seccomp_bpf", "outer_chroot_refused"] {
            assert_eq!(result["evidence"]["sandbox"][field], true);
        }
        assert_eq!(result["evidence"]["browser_version"], "157.0.8103.78");
        assert_eq!(result["evidence"]["origin"], ORIGIN);
        assert_eq!(result["evidence"]["secure_context"], true);
        assert_eq!(result["evidence"]["virtual_authenticator"], true);
        result
    }

    fn finish(&mut self) {
        drop(self.stdin.take());
        let trailing = self
            .output
            .recv_timeout(self.timeout())
            .expect("browser EOF deadline")
            .expect("read browser EOF");
        assert!(
            trailing.is_empty(),
            "browser returned extra protocol output"
        );
        loop {
            if let Some(status) = self.child.try_wait().expect("poll browser driver") {
                assert!(status.success(), "browser driver failed: {status}");
                return;
            }
            assert!(Instant::now() < self.deadline, "browser exit deadline");
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for ConfirmationBrowser {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
        // Give the owned Python process time to remove its authenticator and
        // terminate its own driver group before a final forced reap.
        // SAFETY: the PID belongs to this live, unreaped child process.
        unsafe {
            libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM);
        }
        let cleanup_deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < cleanup_deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct BrowserConfirmationFixture {
    verifier: RegistrationVerifier,
    proof: Proof,
    browser: ConfirmationBrowser,
    request: super::confirmation::FinalAssertionRequest,
    pending: super::confirmation::PendingConfirmation,
    signature: Vec<u8>,
    descriptor: String,
    registration_response: Vec<u8>,
}

fn confirmation_browser_fixture() -> BrowserConfirmationFixture {
    use ed25519_dalek::Signer;
    let (verifier, proof, request, pending) = fixture();
    let mut browser = ConfirmationBrowser::start();
    let created =
        browser.exchange(json!({"version": 1, "action": "register", "options": request.options}));
    assert_eq!(created["status"], "created");
    let mut expected = created["native"].clone();
    for field in ["publicKey", "publicKeyAlgorithm", "authenticatorData"] {
        expected["response"].as_object_mut().unwrap().remove(field);
    }
    assert_eq!(expected, created["wire"]);
    let registration_response = serde_json::to_vec(&created["wire"]).unwrap();
    let registration = verifier
        .finish(pending, &proof, REVIEW, &registration_response)
        .unwrap();
    let descriptor = format!("{:?}", registration.descriptor());
    let commitment = registration.commitment();
    let (context, reserved) = verifier
        .reserve_confirmation(registration, &proof, REVIEW)
        .unwrap();
    // This opaque fixture is not a production proposal canonicalization.
    let proposal_fixture = json!({
        "registration": commitment,
        "context": context,
        "candidate": proof.challenge,
        "candidate_signature": proof.signature,
        "draft": REVIEW,
    });
    let proposal: [u8; 32] = Sha256::digest(serde_json::to_vec(&proposal_fixture).unwrap()).into();
    let binding = super::confirmation::ConfirmationBinding {
        proposal,
        registration: commitment,
        context,
    };
    let (request, pending) = verifier
        .start_confirmation(reserved, &proof, REVIEW, binding)
        .unwrap();
    let signing = ed25519_dalek::SigningKey::from_bytes(
        &crate::enrollment_proof::decode_hex::<32>(
            "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        )
        .unwrap(),
    );
    let signature = signing
        .sign(&Base64UrlUnpadded::decode_vec(&request.signing_message).unwrap())
        .to_bytes()
        .to_vec();
    BrowserConfirmationFixture {
        verifier,
        proof,
        browser,
        request,
        pending,
        signature,
        descriptor,
        registration_response,
    }
}

fn final_browser_assertion(fixture: &mut BrowserConfirmationFixture, verified: bool) -> Json {
    let response = fixture.browser.exchange(json!({
        "version": 1, "action": "assert", "options": fixture.request.options, "verified": verified,
    }));
    fixture.browser.finish();
    if verified {
        assert_eq!(response["status"], "asserted");
        assert_fields(
            &response["native"],
            &[
                "id",
                "rawId",
                "type",
                "response",
                "authenticatorAttachment",
                "clientExtensionResults",
            ],
        );
        assert_fields(
            &response["native"]["response"],
            &[
                "clientDataJSON",
                "authenticatorData",
                "signature",
                "userHandle",
            ],
        );
        assert_eq!(
            response["native"], response["wire"],
            "assertion adapter must preserve every native field"
        );
    }
    response
}

#[test]
fn browser_registered_key_confirms_final_proposal() {
    let mut fixture = confirmation_browser_fixture();
    let response = final_browser_assertion(&mut fixture, true);
    let raw = serde_json::to_vec(&response["wire"]).unwrap();
    let result = fixture
        .verifier
        .finish_confirmation(
            fixture.pending,
            &fixture.proof,
            REVIEW,
            fixture.request.binding,
            &fixture.signature,
            &raw,
        )
        .unwrap();
    assert_eq!(result.binding(), fixture.request.binding);
    assert_eq!(
        result.registration().commitment(),
        fixture.request.binding.registration
    );
    assert_eq!(
        format!("{:?}", result.registration().descriptor()),
        fixture.descriptor
    );
    assert_eq!(
        result.registration().response,
        fixture.registration_response
    );
    assert_eq!(result.assertion(), raw);
    assert_eq!(result.device_signature(), fixture.signature);
    let auth = Base64UrlUnpadded::decode_vec(
        response["wire"]["response"]["authenticatorData"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        result.counter(),
        u32::from_be_bytes(auth[33..37].try_into().unwrap())
    );
    assert_eq!(result.backup_state(), auth[32] & 0x10 != 0);
    assert!(result.counter() > result.registration().descriptor().counter);
}

#[test]
fn browser_final_proposal_substitution_is_refused() {
    let mut fixture = confirmation_browser_fixture();
    let response = final_browser_assertion(&mut fixture, true);
    let mut changed = fixture.request.binding;
    changed.proposal[0] ^= 1;
    assert!(matches!(
        fixture.verifier.finish_confirmation(
            fixture.pending,
            &fixture.proof,
            REVIEW,
            changed,
            &fixture.signature,
            &serde_json::to_vec(&response["wire"]).unwrap()
        ),
        Err(RegistrationError::Refused)
    ));
}

#[test]
fn browser_final_verification_is_required_again() {
    let mut fixture = confirmation_browser_fixture();
    let response = final_browser_assertion(&mut fixture, false);
    assert_eq!(response["status"], "browser-refused");
    assert!(matches!(
        response["error_name"].as_str(),
        Some("NotAllowedError" | "AbortError")
    ));
    assert!(response.get("native").is_none());
    assert!(response.get("wire").is_none());
    assert!(matches!(
        fixture.verifier.finish_confirmation(
            fixture.pending,
            &fixture.proof,
            REVIEW,
            fixture.request.binding,
            &fixture.signature,
            b""
        ),
        Err(RegistrationError::Refused)
    ));
}

fn initial_packet_browser_signing() -> ed25519_dalek::SigningKey {
    // RFC 8032, section 7.1, test 1; public test material only.
    ed25519_dalek::SigningKey::from_bytes(
        &crate::enrollment_proof::decode_hex::<32>(
            "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        )
        .unwrap(),
    )
}

fn initial_packet_browser_input() -> Json {
    let mut challenge = public_proof().challenge;
    challenge.base_revision = 0;
    let proof = Proof::signed(
        challenge,
        &initial_packet_browser_signing(),
        crate::enrollment_proof::current_time().unwrap(),
    )
    .unwrap();
    // This fingerprint is declared fixture data, not verified TLS trust.
    json!({
        "version": 1,
        "profile": "initial-proposal-probe-v1",
        "purpose": "initial-organization",
        "candidate": proof,
        "coordinator": {"endpoint": ORIGIN, "spki_sha256": "99".repeat(32)},
        "webauthn": {
            "rp_id": RP,
            "origin": ORIGIN,
            "registration_profile": "passki-0.5-registration-probe-v1",
            "user_verification": "required"
        },
        "grants": {
            "capabilities": ["manage-members", "manage-projects"],
            "delegation": "organization-model-v2",
            "enrollment_policy_administrator": false,
            "own_additional_device": false
        },
        "project_grants": [],
        "governance_grants": [],
        "recovery": "not-established",
        "content_key_custody": "not-established"
    })
}

fn initial_packet_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn initial_packet_registration_record(registration: &UnconfirmedRegistration) -> Json {
    let descriptor = registration.descriptor();
    json!({
        "commitment": initial_packet_hex(&registration.commitment()),
        "context": {
            "nonce": initial_packet_hex(&registration.context.nonce),
            "issued_at": registration.context.issued_at,
            "expires_at": registration.context.expires_at
        },
        "descriptor": {
            "aaguid": initial_packet_hex(&descriptor.aaguid),
            "counter": descriptor.counter,
            "backup_eligible": descriptor.backup_eligible,
            "backup_state": descriptor.backup_state,
            "algorithm": descriptor.algorithm,
            "public_key": Base64UrlUnpadded::encode_string(&descriptor.public_key),
            "credential": Base64UrlUnpadded::encode_string(&descriptor.credential),
            "transports": descriptor.transports,
            "attachment": descriptor.attachment,
            "opaque_user_handle": Base64UrlUnpadded::encode_string(&descriptor.opaque_user_handle)
        },
        "response": Base64UrlUnpadded::encode_string(&registration.response)
    })
}

struct BrowserInitialPacketFixture {
    verifier: RegistrationVerifier,
    draft: super::confirmation::proposal::Draft,
    registration: UnconfirmedRegistration,
    browser: ConfirmationBrowser,
    input: Json,
    registration_record: Json,
}

fn initial_packet_browser_fixture() -> BrowserInitialPacketFixture {
    assert_eq!(
        std::env::var("VIVAC_BROWSER_LAB").as_deref(),
        Ok("isolated-v1"),
        "browser tests require the isolated lab"
    );
    let input = initial_packet_browser_input();
    let draft =
        super::confirmation::proposal::Draft::parse(&serde_json::to_vec(&input).unwrap()).unwrap();
    let verifier = RegistrationVerifier::new(RP, ORIGIN).unwrap();
    let (request, pending) = verifier.start(draft.proof(), draft.commitment()).unwrap();
    let mut browser = ConfirmationBrowser::start();
    let created = browser.exchange(json!({
        "version": 1, "action": "register", "options": request.options
    }));
    assert_eq!(created["status"], "created");
    let mut expected = created["native"].clone();
    for field in ["publicKey", "publicKeyAlgorithm", "authenticatorData"] {
        expected["response"].as_object_mut().unwrap().remove(field);
    }
    assert_eq!(expected, created["wire"]);
    let registration = verifier
        .finish(
            pending,
            draft.proof(),
            draft.commitment(),
            &serde_json::to_vec(&created["wire"]).unwrap(),
        )
        .unwrap();
    let registration_record = initial_packet_registration_record(&registration);
    BrowserInitialPacketFixture {
        verifier,
        draft,
        registration,
        browser,
        input,
        registration_record,
    }
}

#[test]
fn browser_complete_initial_packet_retains_registration_and_confirms_possession() {
    use ed25519_dalek::Signer;
    let mut fixture = initial_packet_browser_fixture();
    let draft_commitment = fixture.draft.commitment();
    let ready = fixture
        .verifier
        .prepare_proposal(fixture.draft, fixture.registration)
        .unwrap();
    let packet = ready.packet().to_vec();
    let commitment = ready.commitment();
    let context = ready.context();
    let parsed: Json = serde_json::from_slice(&packet).unwrap();
    assert_eq!(
        parsed,
        json!({
            "version": 1,
            "profile": "initial-proposal-probe-v1",
            "draft": fixture.input.clone(),
            "draft_commitment": initial_packet_hex(&draft_commitment),
            "registration": fixture.registration_record.clone(),
            "context": {
                "nonce": initial_packet_hex(&context.nonce),
                "issued_at": context.issued_at,
                "expires_at": context.expires_at
            }
        })
    );
    let mut preimage = b"vivac.bootstrap.proposal-probe.v1\0".to_vec();
    preimage.extend_from_slice(&packet);
    assert_eq!(commitment, <[u8; 32]>::from(Sha256::digest(preimage)));
    let (request, pending) = fixture.verifier.start_proposal(ready).unwrap();
    let binding = request.binding;
    assert_eq!(binding.proposal, commitment);
    assert_eq!(binding.context, context);
    let signature = initial_packet_browser_signing()
        .sign(&Base64UrlUnpadded::decode_vec(&request.signing_message).unwrap())
        .to_bytes();
    let response = fixture.browser.exchange(json!({
        "version": 1,
        "action": "assert",
        "options": request.options,
        "verified": true
    }));
    fixture.browser.finish();
    assert_eq!(response["status"], "asserted");
    assert_eq!(response["native"], response["wire"]);
    let assertion = serde_json::to_vec(&response["wire"]).unwrap();
    let result = fixture
        .verifier
        .finish_proposal(pending, &signature, &assertion)
        .unwrap();
    assert_eq!(result.packet(), packet.as_slice());
    let confirmation = result.confirmation();
    assert_eq!(confirmation.binding(), binding);
    assert_eq!(confirmation.assertion(), assertion.as_slice());
    assert_eq!(confirmation.device_signature(), signature.as_slice());
    assert_eq!(
        initial_packet_registration_record(confirmation.registration()),
        fixture.registration_record
    );
    assert!(confirmation.counter() > confirmation.registration().descriptor().counter);
}

#[test]
fn browser_changed_grants_cannot_prepare_an_initial_packet_from_original_registration() {
    let fixture = initial_packet_browser_fixture();
    let mut changed = fixture.input.clone();
    changed["grants"]["capabilities"] = json!(["grant-project-access", "manage-members"]);
    let changed_draft =
        super::confirmation::proposal::Draft::parse(&serde_json::to_vec(&changed).unwrap())
            .unwrap();
    assert_ne!(changed_draft.commitment(), fixture.draft.commitment());
    assert!(matches!(
        fixture
            .verifier
            .prepare_proposal(changed_draft, fixture.registration),
        Err(RegistrationError::Refused)
    ));
    // Preparation refused before get; the existing bounded Drop path cleans up
    // the owned browser and reaps its driver while it awaits its second request.
    drop(fixture.browser);
}

fn acceptance_browser_organization_signing() -> ed25519_dalek::SigningKey {
    // RFC 8032, section 7.1, test 2; public test material only.
    ed25519_dalek::SigningKey::from_bytes(
        &crate::enrollment_proof::decode_hex::<32>(
            "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
        )
        .unwrap(),
    )
}

fn acceptance_browser_input() -> Json {
    let mut input = initial_packet_browser_input();
    input["version"] = json!(2);
    input["profile"] = json!("initial-acceptance-probe-v1");
    input["acceptance"] = json!({
        "organization_authority_id": "00000000000000000000000040",
        "organization_authority_public_key": initial_packet_hex(
            &acceptance_browser_organization_signing().verifying_key().to_bytes()
        ),
        "principal_acceptance": "candidate-signing-key"
    });
    input
}

fn acceptance_browser_fixture() -> BrowserInitialPacketFixture {
    assert_eq!(
        std::env::var("VIVAC_BROWSER_LAB").as_deref(),
        Ok("isolated-v1"),
        "browser tests require the isolated lab"
    );
    let input = acceptance_browser_input();
    let draft =
        super::confirmation::proposal::Draft::parse(&serde_json::to_vec(&input).unwrap()).unwrap();
    let verifier = RegistrationVerifier::new(RP, ORIGIN).unwrap();
    let (request, pending) = verifier.start(draft.proof(), draft.commitment()).unwrap();
    let mut browser = ConfirmationBrowser::start();
    let created = browser.exchange(json!({
        "version": 1, "action": "register", "options": request.options
    }));
    assert_eq!(created["status"], "created");
    let mut expected = created["native"].clone();
    for field in ["publicKey", "publicKeyAlgorithm", "authenticatorData"] {
        expected["response"].as_object_mut().unwrap().remove(field);
    }
    assert_eq!(expected, created["wire"]);
    let registration = verifier
        .finish(
            pending,
            draft.proof(),
            draft.commitment(),
            &serde_json::to_vec(&created["wire"]).unwrap(),
        )
        .unwrap();
    let registration_record = initial_packet_registration_record(&registration);
    BrowserInitialPacketFixture {
        verifier,
        draft,
        registration,
        browser,
        input,
        registration_record,
    }
}

fn acceptance_browser_expectations(
    commitment: [u8; 32],
) -> super::confirmation::proposal::acceptance::FixtureExpectations {
    use super::confirmation::proposal::acceptance::{FixtureExpectations, FixtureSigner};
    // These are explicitly configured synthetic expectations. Their equality
    // does not accredit an independent organizational channel or human mandate.
    FixtureExpectations::new_fixture(
        "00000000000000000000000001",
        "00000000000000000000000006",
        commitment,
        FixtureSigner::new(
            "00000000000000000000000040",
            acceptance_browser_organization_signing()
                .verifying_key()
                .to_bytes(),
        )
        .unwrap(),
        FixtureSigner::new(
            "00000000000000000000000003",
            initial_packet_browser_signing().verifying_key().to_bytes(),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn browser_fixture_acceptances_and_possession_bind_one_complete_proposal() {
    use ed25519_dalek::Signer;
    let mut fixture = acceptance_browser_fixture();
    let draft_commitment = fixture.draft.commitment();
    let ready = fixture
        .verifier
        .prepare_proposal(fixture.draft, fixture.registration)
        .unwrap();
    let packet = ready.packet().to_vec();
    let commitment = ready.commitment();
    let context = ready.context();
    let parsed: Json = serde_json::from_slice(&packet).unwrap();
    assert_eq!(
        parsed,
        json!({
            "version": 2,
            "profile": "initial-acceptance-probe-v1",
            "draft": fixture.input.clone(),
            "draft_commitment": initial_packet_hex(&draft_commitment),
            "registration": fixture.registration_record.clone(),
            "context": {
                "nonce": initial_packet_hex(&context.nonce),
                "issued_at": context.issued_at,
                "expires_at": context.expires_at
            }
        })
    );
    let mut preimage = b"vivac.bootstrap.acceptance-proposal-probe.v1\0".to_vec();
    preimage.extend_from_slice(&packet);
    assert_eq!(commitment, <[u8; 32]>::from(Sha256::digest(preimage)));
    let expectations = acceptance_browser_expectations(commitment);
    let (requests, pending_acceptances) = fixture
        .verifier
        .begin_fixture_acceptances(ready, expectations)
        .unwrap();
    let organization_message =
        Base64UrlUnpadded::decode_vec(&requests.organization_message).unwrap();
    let principal_message = Base64UrlUnpadded::decode_vec(&requests.principal_message).unwrap();
    let organization_domain = b"vivac.bootstrap.organization-acceptance-probe.v1\0";
    let principal_domain = b"vivac.bootstrap.principal-acceptance-probe.v1\0";
    assert!(organization_message.starts_with(organization_domain));
    assert!(principal_message.starts_with(principal_domain));
    assert_eq!(
        &organization_message[organization_domain.len()..organization_domain.len() + 32],
        commitment.as_slice()
    );
    assert_eq!(
        &principal_message[principal_domain.len()..principal_domain.len() + 32],
        commitment.as_slice()
    );
    assert_ne!(organization_message, principal_message);
    let organization_signature = acceptance_browser_organization_signing()
        .sign(&organization_message)
        .to_bytes();
    let principal_signature = initial_packet_browser_signing()
        .sign(&principal_message)
        .to_bytes();
    let verified = fixture
        .verifier
        .finish_fixture_acceptances(
            pending_acceptances,
            &organization_signature,
            &principal_signature,
        )
        .unwrap();
    assert_eq!(verified.packet(), packet.as_slice());
    assert_eq!(verified.commitment(), commitment);
    let (request, pending) = fixture.verifier.start_fixture_proposal(verified).unwrap();
    assert_eq!(pending.packet(), packet.as_slice());
    assert_eq!(pending.commitment(), commitment);
    let binding = request.binding;
    assert_eq!(binding.proposal, commitment);
    assert_eq!(binding.context, context);
    let device_signature = initial_packet_browser_signing()
        .sign(&Base64UrlUnpadded::decode_vec(&request.signing_message).unwrap())
        .to_bytes();
    let response = fixture.browser.exchange(json!({
        "version": 1, "action": "assert", "options": request.options, "verified": true
    }));
    fixture.browser.finish();
    assert_eq!(response["status"], "asserted");
    assert_eq!(response["native"], response["wire"]);
    let assertion = serde_json::to_vec(&response["wire"]).unwrap();
    let result = fixture
        .verifier
        .finish_fixture_proposal(pending, &device_signature, &assertion)
        .unwrap();
    assert_eq!(result.packet(), packet.as_slice());
    let possession = result.possession();
    assert_eq!(possession.binding(), binding);
    assert_eq!(possession.assertion(), assertion.as_slice());
    assert_eq!(possession.device_signature(), device_signature.as_slice());
    assert_eq!(
        initial_packet_registration_record(possession.registration()),
        fixture.registration_record
    );
    assert!(possession.counter() > possession.registration().descriptor().counter);
    let acceptances = result.acceptances();
    assert_eq!(
        acceptances.organization_message(),
        organization_message.as_slice()
    );
    assert_eq!(
        acceptances.principal_message(),
        principal_message.as_slice()
    );
    assert_eq!(
        acceptances.organization_signature(),
        organization_signature.as_slice()
    );
    assert_eq!(
        acceptances.principal_signature(),
        principal_signature.as_slice()
    );
}

#[test]
fn browser_invalid_organization_acceptance_refuses_before_final_assertion() {
    use ed25519_dalek::Signer;
    let fixture = acceptance_browser_fixture();
    let ready = fixture
        .verifier
        .prepare_proposal(fixture.draft, fixture.registration)
        .unwrap();
    let expectations = acceptance_browser_expectations(ready.commitment());
    let (requests, pending) = fixture
        .verifier
        .begin_fixture_acceptances(ready, expectations)
        .unwrap();
    let organization_message =
        Base64UrlUnpadded::decode_vec(&requests.organization_message).unwrap();
    let principal_message = Base64UrlUnpadded::decode_vec(&requests.principal_message).unwrap();
    let mut organization_signature = acceptance_browser_organization_signing()
        .sign(&organization_message)
        .to_bytes();
    organization_signature[0] ^= 1;
    let principal_signature = initial_packet_browser_signing()
        .sign(&principal_message)
        .to_bytes();
    assert!(matches!(
        fixture.verifier.finish_fixture_acceptances(
            pending,
            &organization_signature,
            &principal_signature
        ),
        Err(RegistrationError::Refused)
    ));
    // An invalid statement refuses before get; the existing bounded Drop path
    // cleans up the owned browser while it awaits its second request.
    drop(fixture.browser);
}
