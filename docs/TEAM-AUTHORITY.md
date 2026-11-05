# Shared authority and local capture

This is the contract for future team collaboration. It separates the local
work history from shared permissions and governance so that recording work
never waits for the network. It defines acceptance requirements, not an
implemented server, transport, authentication system or command.

## Identities and resources

A person, automation agent, device, session, project and lane have distinct
opaque identities. A lane remains a working folder with its own focus and
session state; it is neither a person nor a permission group. An existing
local actor does not establish an authenticated person's identity. Historical
activity without that evidence remains unattributed.

A sharing domain identifies the resources and revisions an audience may
receive. Its audience can span several lanes or cover selected work from one
lane. Project membership does not automatically open a restricted domain.
Adding a person to a project does not create a lane or grant access to other
projects. Human identity mappings belong in a protected directory, not in
provenance prose or event actor fields.

## Separate authority records

The local event log remains the source of the local work history. Its append
records work and proposals, not proof of shared authorization. Authority
records establish membership, delegated capabilities, active governance
revisions and revocations. Locally cached authority records are verified
replicas with an explicitly known revision, not a second editable memory map.

The first shared protocol has one online admission coordinator per organization.
It orders authority transitions against an exact expected predecessor.
It must verify the participant's authority and the reviewed transition;
receiving a proposal alone cannot activate it. The coordinator may order and
replicate valid transitions but cannot manufacture participant approval.
It does not receive content decryption keys.

Each admitted transition binds the organization, affected scope, predecessor,
operation identity, complete reviewed envelope and authenticated authorizing
principal. The exact encoding, signature scheme and credential mechanisms
must be specified and reviewed before implementation. No individual secret
fingerprint or personal identity is added to the work log for this purpose.

Only one transition can succeed against a predecessor. A concurrent proposal
based on that predecessor remains a conflict requiring review. An authorized
requester retrying the same operation and envelope receives its existing
result; reusing the operation identity with different bytes refuses.
A lost response is reconciled before
resubmission when the requester still has authority to read its result.
Revocation can remove that access; reconciliation is not a revoked credential's
exception to current authorization. A separately authorized auditor or recovery
flow may inspect retained evidence without impersonating the revoked actor.
Admitted authority must be durable before acknowledgement; a
restart cannot silently restore an earlier authorization state.
An idempotent replay returns historical evidence, never a renewed permission.
Using that result for another publication or delivery still requires current
admission, including after the original principal has been revoked.

Clients verify authority provenance and retain their last accepted revision.
A lower or conflicting revision is not silently accepted. A fresh device or
restored installation needs an authenticated bootstrap and a current trusted
checkpoint; a backup is not evidence that its old permissions still apply.
The organization establishes its initial trust checkpoint during reviewed
bootstrap. Subsequent device enrollment requires an existing trusted client
with explicit enrollment authority, bound to the new device and a current
coordinator checkpoint. A restored backup or a successful login alone cannot
establish that trust or release content keys. Loss of all trusted clients is
a separate recovery case, not permission to bootstrap from stale state.
The coordinator is trusted for live ordering and online access checks.
Authenticated records alone do not prove global freshness or detect every
fork presented to a new client. This design does not promise Byzantine
consensus or availability against a malicious coordinator. Detected rollback,
conflicts or unverifiable authority prevent new shared admission, while local
capture remains usable.

## Local and shared states

| State | What it establishes |
|---|---|
| Saved locally | The local history contains the work or proposal. No remote receipt is claimed. |
| Pending admission | An explicit sharing or governance request exists but has no confirmed current authorization. |
| Admitted | The shared authority accepted the exact operation under a recorded revision. |
| Received by a client | That client obtained and verified the specified revision. This does not prove that a model followed it. |
| Conflicted or refused | Shared admission did not complete. Local work remains local; the refusal grants no access. |

An admission receipt is not a payload-custody or delivery receipt. Content
publication must separately establish complete verified retention and current
authorization before claiming availability. These states must not be collapsed
into a generic synchronization success message.

Ordinary capture does not perform network requests or wait for admission.
Synchronization and privileged administration are separate, explicitly enabled
operations. Disconnected clients report the revision they know and cannot
claim that it is globally current. Offline proposals never become active
shared rules or new grants by themselves.

Before admitting pending work, the current authority checks the principal,
scope, delegation, reviewed revision and audience again. A stale review is
not refreshed and applied under the old confirmation. Work produced under an
older rule keeps that historical fact; whether it may now be shared needs a
new assessment against the current applicable authority.

## Governance and delegation

Proposing a rule, ratifying a rule and ratifying a pillar are separate
capabilities. Operating infrastructure, managing members, reading content,
delegating privileges and performing key recovery are also separate.
Managing members cannot grant its holder governance or recovery privileges.
Delegation cannot exceed the grantor's permitted actions or scope.
Initial authorities and their delegation limits are established during
explicit organization bootstrap, before ordinary invitations are accepted.
Redelegation requires an explicit capability. Every link in a delegation chain
must remain valid; revoking an intermediate grant invalidates its downstream
authority for new admission. Cycles, unverifiable links and chains beyond a
declared depth limit refuse rather than truncating the authorization check.

An automation credential cannot ratify a norm, including when its delegating
person could do so. Ratification requires a separate interactive flow with
fresh, non-delegable user verification. The confirmation is limited-use and
bound to the exact reviewed envelope, predecessor, scope and audience.
Neither an actor field nor an ordinary user session supplied by an agent is
evidence of that confirmation. The verification mechanism must be accredited
before this flow is exposed; it proves an authorization interaction, not
that the person understood the proposal.
Consuming the confirmation and admitting its exact operation must be atomic.
One confirmation cannot ratify two operation identities. A retry of the
original operation reconciles its receipt without consuming the confirmation
again or granting another action; a changed envelope requires new verification.

Norms progress from draft to proposed, active, superseded or retired.
Activation and replacement append authority records; they do not rewrite
the rule that governed earlier work. Imported norms need review rather than
invented historical approvals. The product's security and integrity guarantees
cannot be weakened by an organization or project norm. Conflicting norms do
not resolve through last-writer-wins behavior.

Members receive the applicable active governance revision for their work.
Delivery records identify the revision and scope actually delivered. After
establishing a verified baseline, clients may deliver relevant changes and
references instead of repeating unrelated history. Missing baseline data or
insufficient context must not be reported as complete instruction delivery.
Tools retain their native capabilities; uncertainty is not permission to
disable unrelated harness features.

## Audience and history

Reading content does not authorize redistributing it. Publication requires
source reading rights, source redistribution authority, destination publishing
rights and review of the exact revision and audience. Restricted history,
artifacts and derived context are not included merely because a visible node
references them.

New membership does not grant previously published history by default.
Historical access is an explicit reviewed grant identifying its scope and
revisions. Future publications use the current audience policy. A group or
membership change cannot silently expand an old historical grant. Revoking
membership and changing key distribution must be coordinated before new
content is published for the remaining audience.

Search, briefs, reports, previews, downloads and context delivery apply the
same source authorization. Partial views do not reveal private ancestor IDs,
titles, hashes, counts, paths or reasons. They state their visible scope
without pretending to be a complete canonical tree. If closure or integrity
depends on hidden work, a partial view cannot certify global completion;
verification requires the necessary authority and visibility or remains pending.

## Revocation and in-flight delivery

Revocation takes effect when its authority transition is durably admitted.
A revocation acknowledgement identifies that revision. Starting a new remote
operation, admitting pending work or authorizing another delivery unit must
check against that revision or a later one. A previously issued credential,
review or signature does not override the current revocation.

The check and admission of a delivery unit must be ordered with revocation,
so that a check performed before revocation cannot authorize new units after
it. Units already admitted before the revocation may still be in flight.
Their byte and lifetime limits must be fixed, tested and disclosed before
remote delivery is enabled. An entire unbounded download cannot be treated
as one previously authorized unit.
Limits must also bound aggregate in-flight bytes and units by principal,
sharing domain and organization. Concurrent devices cannot each obtain an
independent unlimited budget. Capacity is reserved before admission and
released on completion or expiry; revocation stops new reservations. The
remaining exposure bound must account for all admitted units, not just one.

The first protocol does not expose bearer download URLs that bypass those
checks. Retries, resumed transfers and range requests require fresh admission.
Loss of current authority refuses another delivery unit rather than using a
stale permission cache. Revocation cannot retrieve bytes already received,
erase prior keys from another device or remove context already sent to a model.

Revocation of network access and exclusion from future encryption epochs are
distinct requirements. New content must not reuse distribution that includes
the removed participant. Rotation does not make previously received content
secret again. The key protocol must establish these properties before a
collaboration implementation can claim them.

## Trusted clients and recovery

Content is decrypted by authorized installed clients controlled by the owner
or organization. The synchronization operator cannot obtain keys merely by
assigning itself a database role. Client distribution and updates are part of
the trust boundary: server-supplied web code is not automatically a trusted
place to handle keys. The local loopback reading surface may use the installed
client; remote administration only receives explicitly authorized information.

An operator that also controls endpoints or trusted client updates is inside
that trust boundary. Recovery custodians, if enabled, are also trusted for
their granted scope. The product must not claim privacy from such an authority.

Account recovery and content-key recovery are different operations. Neither
a password reset nor a future corporate identity-provider login grants
decryption keys by itself. The recovery policy, custodian powers, bootstrap,
lost-device procedure and key lifecycle remain prerequisites for implementation.
No default recovery authority is inferred by this contract.

## Deployment boundary

The same contract applies to self-hosted and future managed installations.
Its security controls must not require a vendor-hosted account or a proprietary
service. Initial team authentication must work without a corporate identity
provider. Internal principal identities remain independent of the login
mechanism so that corporate identity providers can be added later without
rewriting provenance. Local credentials and their lifecycle still require a
reviewed implementation; a VPN is not application authentication.

## Executable policy model

`src/team_authority/` is an in-memory policy model compiled only for tests.
It models organization bootstrap, opaque principals, separate organization
capabilities and project permissions, revision conflicts, revocation and
idempotent operation results. A bootstrap administrator does not implicitly
receive content access. Revoked identities cannot be silently enrolled again.
An active author can reconcile its own exact operation as historical evidence;
an inactive author is refused before receipt lookup.
Capability and permission changes here are direct organization-policy
assignments. They do not model delegated credentials or establish revocation
propagation through a credential chain. Project-access assignment authority
is explicit; managing members alone does not provide it.

Its inputs are scenario identities and typed operations, not authenticated
requests. Exact operation equality in this model does not define a signed wire
encoding. It has no credentials, human-verification flags, invitations carrying
secrets, key recovery, durable storage, network or public command. Ratification
is not enabled. A future authentication and admission layer must bind the
actual principal and reviewed request before using policy eligibility.

The model bounds principals, including revoked identities, at 128, projects
at 64 and admitted operations at 1,024. Hitting a limit refuses the next
transition without dropping history or partially changing authority. These
are model bounds, not measured production capacity or latency claims.

Its tests establish policy semantics and in-memory transition behavior only.
They do not establish secure enrollment, authentication, durable admission,
encrypted sharing or readiness for a team deployment.

## Acceptance cases

These are requirements for future integration tests, not executed security
claims. They must be exercised through actual CLI, MCP, API and web paths,
including derived data and artifact delivery; a role-table test alone is insufficient.

| Case | Required result |
|---|---|
| Local capture while the coordinator is unreachable | Work is saved locally without claiming shared admission. |
| Two proposals use the same authority predecessor | At most one activates; the other requires conflict review. |
| Retry after acknowledgement was lost | Reconcile the existing exact result; do not repeat a different transition. |
| Replay an old successful receipt after revocation | Historical result only; no renewed publication or delivery permission. |
| A revoked actor requests reconciliation | Ordinary receipt access is refused; the retained result is not erased. |
| Restore an old authority cache or backup | Do not revive revoked permissions or active norms without a current verified checkpoint. |
| Infrastructure or member administrator attempts privilege escalation | No implicit content, governance or recovery access. |
| Agent uses a privileged person's delegated credential to ratify | Refused without the separate interactive confirmation. |
| Reuse one human confirmation for two operations | At most the original exact operation is admitted; consumption and admission are atomic. |
| Three principals redelegate across a revoked intermediate grant | No downstream admission, scope expansion or cycle bypass. |
| Reviewed content, predecessor, scope or audience changes | Old confirmation cannot activate the changed envelope. |
| Add a member after publication | No historical content without its explicit reviewed grant. |
| Reader attempts to publish into another scope | Source redistribution and destination publishing rights are both required. |
| Private ancestors appear behind visible work | No leakage through references, counts, search or reports; no false global closure. |
| Revocation wins the ordering race | No new delivery units or pending publications admitted with old authority. |
| Transfer started before revocation | Only already admitted bounded units may remain in flight; retries reauthorize. |
| Concurrent devices admit many units before revocation | Aggregate principal, domain and organization budgets bound all remaining in-flight exposure. |
| Authority becomes unavailable during transfer | Another unit is refused without changing the local work history. |
| A removed participant still has old bytes and keys | Report that limit; prevent future authorized delivery and key inclusion. |
| A client or model receives a governance revision | Record reception without claiming compliance or understanding. |

## Protocol review references

[RFC 7009](https://www.rfc-editor.org/rfc/rfc7009.html) describes token
revocation and propagation considerations. [SpiceDB's consistency guidance](https://authzed.com/docs/spicedb/concepts/consistency)
illustrates why authorization freshness must be chosen explicitly. Neither
reference selects an authentication system or authorization dependency for vivac.
