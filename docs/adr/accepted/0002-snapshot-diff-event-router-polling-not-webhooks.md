# ADR-0002: Snapshot-diff event router, polling not webhooks

> State: Accepted

## Status

Accepted

## Stakeholders

conduit maintainers. Affected: every forge adapter author (the contract this
decision pins is what they implement against) and partner teams whose forges
sit on private networks.

## Context and Problem Statement

conduit must react to forge activity — a human labelling an issue, a review
being submitted, a PR merging — identically on GitHub and on a self-hosted
Gitea. Event APIs are where forges diverge worst: webhook payloads, event
feeds, and delivery semantics differ per forge and per version. If each
adapter translated its forge's native events, "identical behaviour" would be a
per-adapter promise that drifts. The spike also runs against a localhost
container and a read-only GitHub, where inbound webhook delivery is either
impossible or unwanted.

## Decision Drivers

- Event semantics must be defined once, not once per adapter — neutrality by
  construction, provable by a shared conformance suite
- Partners' forges sit on private networks: zero inbound exposure is a
  feature, not a limitation
- Adapters should stay thin: one normalized read is easier to implement and
  to conformance-test than an event protocol
- Restart safety: the event source must replay cleanly from persisted state

## Considered Options

- Per-forge webhook receivers translating native payloads to internal events
- Per-forge event-feed polling (each adapter parses its forge's event API)
- Snapshot polling with one shared pure diff deriving all events

## Decision Outcome

Chosen: **snapshot polling with one shared pure diff**, because it moves all
event semantics into a single pure function that every forge exercises
identically.

Adapters implement only `fetch_snapshot()` — one normalized read of
conduit-labeled issues and `conduit/*`-branch PRs. A single pure
`diff(prev, next) -> Vec<ForgeEvent>` in `forge/mod.rs` derives the five event
kinds (`IssueLabeled`, `ReviewSubmitted`, `CiChanged`, `PrMerged`,
`PrClosed`); reviews dedupe on forge-native review ids so an edited review
never re-fires. The cursor is the previous `RepoSnapshot`, persisted per
forge; it advances only after a tick's actions complete. A webhook receiver
remains a future *second producer* of the same `ForgeEvent` stream feeding an
unchanged router.

### Positive Consequences

- GitHub and Gitea behave identically by construction; the conformance suite
  asserts it instead of sloganeering
- Works against localhost containers and read-only forges with no inbound
  ports, tunnels, or delivery retries
- A lost cursor degrades gracefully: the first tick replays everything once,
  behind the idempotency probes

### Negative Consequences

- State that flaps within one poll interval (a review submitted then
  dismissed) is invisible by design — a documented contract, acceptable
  because merge stays a human gate
- Reaction latency is bounded by the poll interval
- Adapters carry snapshot obligations (keep terminal items visible, paginate
  explicitly, unique ids) that the diff cannot enforce for them

## Implementation

<!-- adroit:plan -->

### Implementation Plan: Snapshot-diff event router, polling not webhooks (ADR-0002)

#### Step 1: Design and Prototyping
* File: `forge/mod.rs`
* Tasks:
	+ Implement a single pure function `diff(prev, next) -> Vec<ForgeEvent>`
	+ Define the five ForgeEvent types (`IssueLabeled`, `ReviewSubmitted`, etc.)
	+ Establish the cursor concept (RepoSnapshot) for persisted state per forge
* Rationale: This step involves defining the core logic of the event router and its components.

#### Step 2: Adapter Implementation
* File: Conduit adapters
* Tasks:
	+ Implement `fetch_snapshot()` for each adapter, using the shared `diff` function
	+ Ensure adapter-specific behavior is minimal and adheres to the contract
* Rationale: Adapters need to interact with the event router; this step ensures seamless integration.

#### Step 3: Testing and Validation
* File: Integration tests
* Tasks:
	+ Write comprehensive integration tests for each aspect of the event router (diff, cursor, etc.)
	+ Validate that adapters produce the expected ForgeEvents
	+ Ensure no forge-specific behavior is introduced through adapter implementation
* Rationale: Thorough testing ensures the event router behaves correctly and meets its requirements.

#### Step 4: Rollout and Migration
* Tasks:
	+ Prepare for rollout by updating documentation, API references, and example code
	+ Gradually migrate adapters to the new event router implementation
	+ Monitor migration progress and address any issues promptly
* Rationale: A controlled rollout allows for a smooth transition from the previous event router implementation.

#### Step 5: Deployment and Maintenance
* Tasks:
	+ Deploy the updated Conduit application with the new event router implementation
	+ Ensure continuous monitoring of the system's behavior, addressing any regressions or issues promptly
	+ Perform regular maintenance tasks to prevent drift in event semantics
* Rationale: Ongoing monitoring and maintenance are crucial for ensuring the long-term stability and reliability of the event router.

#### Risks and Considerations
* Flapping state within a single poll interval: Acceptable because merge stays a human gate, but monitor system behavior closely.
* Reaction latency: Bounded by the poll interval; ensure reasonable performance characteristics.
* Adapter-specific obligations: Ensure minimal adapter-specific behavior is introduced through implementation.

### Test Frameworks and Tools

* [Cargo testing](https://doc.rust-lang.org/cargo/testing/index.html) for integration tests
* [Testcontainers](https://testcontainers.github.io/) for local environment simulation

### Deployment Strategy

* Gradual rollout of adapters to the new event router implementation, with continuous monitoring and support.
* Deployment of the updated Conduit application to production environments.

<!-- /adroit:plan -->
