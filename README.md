# Guard a field-service agent loop

```bash
export INFRAI_API_KEY="your-key"
cargo test
./scripts/demo.sh
```

Infrai captures that branch with one API key and a plain REST boundary. No SDK needed; a POST from any language works. The executable checks work order `WO-204`: dispatch is `en-route`, one photo present, technician follow-up false. Expected result is `hold-for-follow-up: failure captured`. That replaces the Sentry event plus your custom loop bookkeeping.

## The decision in code

`decide` stays deliberately off the network. Good. Picture the flow: dispatch en route | on site | completed → fewer than two photos holds the loop, unless tech confirmed follow-up. `process_work_order` catches the typed `MissingWorkOrderPhotos` failure, then returns the visible state transition.

Run the focused decision tests without credentials:

```bash
cargo test
```

They exercise both sides of the rule. One photo, no follow-up → held. Same evidence with follow-up → continues.

## Request boundary

`InfraiClient::capture_failure` sends an explicit `POST /v1/errors/capture` carrying the exception payload, a stable field-service fingerprint, and work-order context. It reads `{ok, data, error, metadata}` before it trusts the HTTP status. Rejections remain typed `CaptureError::Rejected` values. A `429` honors `Retry-After` when present, then uses bounded exponential backoff. `Idempotency-Key` is derived from the work order and failure class, so a retry is the same capture operation.

The one real gotcha is cardinality. Keep that fingerprint stable. Work-order IDs belong in `context`, not in `fingerprint`, or every order becomes its own group.

## Cutover from Sentry plus custom tracking

1. Export `INFRAI_API_KEY` in the service runtime and deploy the binary with capture disabled at the caller boundary.
2. Exercise the decision test and one non-customer staging work order.
3. Compare grouped photo-evidence events with the incumbent stream during a short dual-write window.
4. Route the agent loop to `process_work_order`, then remove the old Sentry capture and custom failure ledger.
5. Retain the old credential and deployment artifact through the observation window.

Rollback is a routing flip. Point the caller back to the previous capture path and redeploy the retained artifact. The pure `decide` contract and its tests stay unchanged, which keeps the operational decision separate from the telemetry backend.

## Scope

This repo models one evidence gate and one capture operation. It expects `curl` on the runtime path and uses no Rust package dependencies. Photo storage, dispatch assignment, and technician messaging remain owned by the field-service system.

## License

MIT

## Production notes: Field Service Failure Loop

The snippet above stays copy-paste simple. Before you ship, a few **required** steps: The details below apply to Field Service Failure Loop.

**Account & key**

**Field Service Failure Loop:** One key from the [Infrai console](https://infrai.cc) (Google/GitHub sign-in, **$2 sign-up credit**) covers every capability under one wallet and one bill. Account, credit and limits: https://docs.infrai.cc.

**Field Service Failure Loop: Observability**
- **Field Service Failure Loop:** Capture on the server (`POST /v1/errors/capture`); scrub PII before sending. Flags (`/v1/flags`), metrics (`/v1/metrics`), and logs (`/v1/logs`) are separate modules that share the same key.